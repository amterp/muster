//! A window lists the tabs it holds, against a real daemon (kan a_2Mhi0EZlv).
//!
//! The rules are pinned in `tab-holding.json`. What needs a daemon is everything around them:
//! that a window comes back to the tabs it held, that one which has gone keeps them, and that
//! tabs already running are taken by the first window to open. Two windows at once are
//! `windows.rs`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use muster::proto::{
    AttachPane, CreateTab, Event, OpenWindow, ReadWindow, Request, Response, Startup, ViewChanged,
    event, request, response,
};
use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::composition::{DaemonId, HeldWindow, WindowName};
use muster_core::mirror::backend::TabId;
use muster_daemon_proto::{self as daemon_proto, Side, session_request};
use muster_harness::requests::{beside, create, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

/// A window that has gone keeps its tabs, and a window opened beside it does not take them. Gone by
/// quitting here, which leaves its row naming a process that is not there - the state a window in
/// another process is in once it has ended, and one somebody closed is in too.
#[test]
fn a_window_that_has_gone_keeps_its_tabs() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    open_a_window(&daemon, "window-1");
    let theirs = until_showing_something();
    assert_ok(&answer(request::Payload::Quitting(muster::proto::Quitting::default())));

    turn.relaunch();
    open_a_window(&daemon, "window-2");
    let ours = until_showing_something();

    assert_ne!(ours, theirs, "the new window opened onto the closed window's tab");
    assert!(!listed().contains(&theirs), "the new window lists the closed window's tab");
    let record = holders(&daemon);
    assert!(
        record.iter().any(|(tab, window)| tab == &theirs && window == "window-1"),
        "the closed window lost its tab: {record:?}"
    );
}

/// The first launch after this record existed comes back exactly as the window was left.
///
/// Every tab is held by nobody then, and the window's arrangement lists every tab it had. A
/// window that took only what it could prove was its own would come back to less than it was
/// left with, which for a single window - most people - is a regression with nothing to show
/// for it.
#[test]
fn the_first_launch_with_a_record_takes_every_tab_it_was_left_with() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    open_a_window(&daemon, "window-1");
    until_showing_something();
    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        take_focus: true,
        ..CreateTab::default()
    })));
    until(
        "the second tab to arrive",
        || listed().len() == 2,
        || format!("this window lists {:?}", listed()),
    );
    let before = listed();
    assert_ok(&answer(request::Payload::Quitting(muster::proto::Quitting::default())));

    // What a machine upgrading to this looks like: an arrangement, and no record beside it.
    std::fs::remove_file(record(&daemon)).expect("the window wrote a record");
    turn.relaunch();
    open_a_window(&daemon, "window-1");
    until(
        "the window to come back onto both of its tabs",
        || listed() == before,
        || format!("this window lists {:?} and was left with {before:?}", listed()),
    );
}

/// A record deleted while the window is open costs the window nothing.
///
/// The warning for a record that cannot be read tells somebody to delete it, so deleting it has
/// to be safe. The app keeps the record in memory and writes it whole, so the next change puts
/// back every window and every tab.
#[test]
fn a_record_deleted_under_an_open_window_is_written_again() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    open_a_window(&daemon, "window-1");
    let ours = until_showing_something();

    std::fs::remove_file(record(&daemon)).expect("the window wrote a record");
    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        take_focus: true,
        ..CreateTab::default()
    })));

    assert!(listed().contains(&ours), "the window let go of its tab when the record went");
    assert!(
        holders(&daemon).contains(&(ours, "window-1".to_string())),
        "the next change did not write the window and its tab back into the record: {:?}",
        holders(&daemon)
    );
}

/// A first window lists every tab the daemon already holds.
///
/// Alex's upgrade: his first launch of this release finds every tab already running and no record
/// saying whose. The daemon described them before this window had said it was open, so no window
/// could take them then, and nothing asked again once it had.
#[test]
fn a_first_window_lists_every_tab_already_on_the_daemon() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    tabs_already_running(&daemon);

    open_a_window(&daemon, "window-1");

    until(
        "the window to list both tabs the daemon already held",
        || listed().len() == 2,
        || format!("this window lists {:?}; the record says {:?}", listed(), holders(&daemon)),
    );
}

/// The same, for a launch that names a pane: `muster <pane>`, which the contract tier runs.
#[test]
fn a_first_window_opened_onto_a_pane_lists_every_tab_already_on_the_daemon() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    tabs_already_running(&daemon);
    start_a_window(&daemon, "window-1");

    assert!(matches!(
        answer(request::Payload::AttachPane(AttachPane { pane_id: "p1".to_string() })).payload,
        Some(response::Payload::Attached(_))
    ));

    until(
        "the window to list both tabs the daemon already held",
        || listed().len() == 2,
        || format!("this window lists {:?}; the record says {:?}", listed(), holders(&daemon)),
    );
    assert_eq!(
        holders(&daemon).iter().filter(|(_, window)| window == "window-1").count(),
        2,
        "the record does not say this window holds both: {:?}",
        holders(&daemon)
    );
}

/// A window reopened lists a tab made while no window was open.
///
/// Nobody held it, and this window is the only one open, so it is this window's. The daemon
/// described it before the window had said it was open again.
#[test]
fn a_reopened_window_lists_a_tab_made_while_it_was_closed() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    open_a_window(&daemon, "window-1");
    until_showing_something();
    assert_ok(&answer(request::Payload::Quitting(muster::proto::Quitting::default())));
    a_tab_made_outside_muster(&daemon, "t-outside");

    turn.relaunch();
    open_a_window(&daemon, "window-1");

    until(
        "the reopened window to list the tab made while it was closed",
        || listed().len() == 2,
        || format!("this window lists {:?}; the record says {:?}", listed(), holders(&daemon)),
    );
}

/// A tab this window holds on a daemon that has not answered yet stays this window's.
///
/// The record forgets a tab no daemon describes, unless the window holding it follows a daemon
/// that has not answered, which may be where the tab is. Were a daemon still attaching counted as
/// not followed, this window would give up its own tab on a slow devenv the moment it opened,
/// leaving it to whichever window came to the front next. Staged with a daemon slow to send its
/// state, which is followed from the start; a daemon not yet followed at all is
/// `daemon_still_starting.rs`, in a binary of its own.
#[test]
fn a_tab_on_a_daemon_still_attaching_stays_this_windows() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    a_tab_made_outside_muster(&daemon, "t-slow");
    // The record as a launch left it: this window, closed, holding the tab.
    let path = record(&daemon);
    let mut holders = read_record(&path);
    holders.opened(HeldWindow {
        name: WindowName::new("window-1"),
        arrangement: daemon.root().join("window-1.toml").to_string_lossy().into_owned(),
        socket: String::new(),
        pid: 1,
        install: String::new(),
        focused: 0,
        daemons: std::iter::once(DaemonId::new("local")).collect(),
    });
    holders.take(TabId::new("t-slow"), &WindowName::new("window-1"));
    holders.closed(&WindowName::new("window-1"));
    write_record(&path, &holders);

    let relay = daemon.delaying_answers_where(subscribes, std::time::Duration::from_secs(5));
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: relay.muster_config().to_string_lossy().into_owned(),
        state_path: daemon.root().join("window-1.toml").to_string_lossy().into_owned(),
        tab_holders_path: path.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

    let record = holders_in(&path);
    assert!(
        record.iter().any(|(tab, window)| tab == "t-slow" && window == "window-1"),
        "the window let go of its tab on a daemon still attaching: {record:?}"
    );
    drop(relay);
}

/// A record a newer Muster wrote is left as it is: this one cannot read it, and writing back the
/// little it could would throw away which window holds every tab, for the newer one too.
#[test]
fn a_record_from_a_newer_muster_is_not_written_over() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let path = record(&daemon);
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("writable");
    let newer = "version = 99\n\n[[window]]\nname = \"window-9\"\n";
    std::fs::write(&path, newer).expect("writable");

    open_a_window(&daemon, "window-1");
    until_showing_something();

    assert_eq!(
        std::fs::read_to_string(&path).ok().as_deref(),
        Some(newer),
        "the newer Muster's record was written over"
    );
}

/// The window's subscribe, whose answer carries the daemon's state.
fn subscribes(request: &daemon_proto::Request) -> bool {
    matches!(
        &request.service,
        Some(daemon_proto::request::Service::Session(daemon_proto::SessionRequest {
            request: Some(session_request::Request::Subscribe(_)),
        }))
    )
}

/// Every tab the record names, with the window holding it.
fn holders(daemon: &Daemon) -> Vec<(String, String)> {
    holders_in(&record(daemon))
}

fn holders_in(path: &Path) -> Vec<(String, String)> {
    let holders = read_record(path);
    holders
        .windows()
        .flat_map(|window| {
            holders.held_by(&window.name).map(|tab| (tab.to_string(), window.name.to_string()))
        })
        .collect()
}

fn record(daemon: &Daemon) -> PathBuf {
    daemon.root().join("holding/tabs.toml")
}

fn read_record(path: &Path) -> muster_core::composition::Holders {
    from_toml(&std::fs::read_to_string(path).unwrap_or_default())
        .expect("the record this window writes reads back")
}

fn write_record(path: &Path, holders: &muster_core::composition::Holders) {
    std::fs::create_dir_all(path.parent().expect("the record is in a directory"))
        .expect("the record's directory can be made");
    std::fs::write(path, to_toml(holders)).expect("the record can be written");
}

fn open_a_window(daemon: &Daemon, name: &str) {
    start_a_window(daemon, name);
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
}

/// Everything a launch does before it says what to show: `muster` then opens, and `muster <pane>`
/// attaches that pane instead.
fn start_a_window(daemon: &Daemon, name: &str) {
    *VIEW.lock().expect("a panicking test poisoned the view") = None;
    muster::ffi::muster_set_event_callback(Some(note));
    let arrangement = daemon.root().join(format!("{name}.toml"));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: arrangement.to_string_lossy().into_owned(),
        tab_holders_path: record(daemon).to_string_lossy().into_owned(),
        ..Startup::default()
    })));
}

/// Two tabs on the daemon before any window opens: one split three ways, and one of a single pane.
///
/// What the contract tier's split check stages (`crates/muster-contract/tests/launch.rs`), and
/// what Alex's machine looks like at his first launch of the release that brought this record:
/// every tab already there, and no record saying whose.
fn tabs_already_running(daemon: &Daemon) {
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", Side::Right)));
    make(&mut control, create("p3", beside("p2", Side::Down)));
    make(&mut control, create("p4", in_new_tab("t2")));
}

/// A tab somebody made by talking to the daemon directly rather than through a window, which is
/// what a script or another client does.
fn a_tab_made_outside_muster(daemon: &Daemon, tab: &str) {
    make(&mut daemon.connect(), create(&format!("p-{tab}"), in_new_tab(tab)));
}

fn until_showing_something() -> String {
    until(
        "the window to open onto a tab",
        || showing().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    showing().expect("just waited for it")
}

fn showing() -> Option<String> {
    let view = latest_view()?;
    let region = view.regions.first()?;
    region.root.as_ref()?;
    Some(region.tab_id.clone())
}

/// The tabs this window lists, in its order.
fn listed() -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => window
            .roster
            .iter()
            .flat_map(|roster| roster.tabs.iter())
            .map(|tab| tab.tab_id.clone())
            .collect(),
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::ViewChanged(view)) = event.payload {
        *VIEW.lock().expect("a panicking test poisoned the view") = Some(view);
    }
}

fn latest_view() -> Option<ViewChanged> {
    VIEW.lock().expect("a panicking test poisoned the view").clone()
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(
            response::Payload::Ok(_) | response::Payload::Made(_) | response::Payload::Opened(_),
        ) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
