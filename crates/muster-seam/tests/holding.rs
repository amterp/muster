//! A window lists the tabs it holds, against a real daemon (kan a_2Mhi0EZlv).
//!
//! The rules are pinned in `tab-holding.json`. What needs a daemon is everything around them:
//! that a tab this window asks for is recorded as its own even while another window is in front,
//! that a tab made outside Muster goes where the rule says, and that a tab another window takes
//! leaves this one.
//!
//! The seam holds one session per process, so the other window is a stand-in: a socket this test
//! listens on, named in the shared record the way a real window names itself. Dialing it is what
//! tells this window it is open, which is the same question a real window answers.

use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use herdr_harness::{Daemon, until};
use muster::proto::{
    ArrangePane, AttachPane, CreateTab, Event, FocusPane, OpenWindow, ReadTabHolders, ReadWindow,
    Request, Response, Startup, ViewChanged, WindowFocus, event, request, response,
};
use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::composition::{DaemonId, HeldWindow, WindowName};
use muster_core::mirror::backend::TabId;
use prost::Message;
use serde_json::json;

/// A tab made outside Muster joins the window that was in front, and only that one.
///
/// Alex's rule for a tab nothing asked for: the window somebody was last looking at is where they
/// will look for it. Before this, every window listed it, and a window with nothing chosen opened
/// onto it.
#[test]
fn a_tab_made_outside_muster_joins_the_window_in_front() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start();

    // Another window, open and in front of this one.
    let other = another_window(&daemon, "window-9", i64::MAX / 2);
    open_a_window(&daemon, "window-1");
    let ours = until_showing_something();

    daemon.call("tab.create", &json!({ "focus": false }));
    // The stand-in takes nothing itself, so what this can see is the half that matters here:
    // this window heard of the tab and left it alone.
    until(
        "this window to hear of the tab nobody asked for",
        || panes_on_the_daemon() == 2,
        || format!("the window has heard of {} panes", panes_on_the_daemon()),
    );
    assert_eq!(listed(), vec![ours.clone()], "this window took a tab the window in front was due");
    assert!(
        holders(&daemon).iter().all(|(tab, _)| tab == &ours),
        "this window recorded a tab it was not due: {:?}",
        holders(&daemon)
    );
    drop(other);

    // And the other way round: with this window in front, the next one is its own.
    turn.relaunch();
    let _other = another_window(&daemon, "window-9", 0);
    open_a_window(&daemon, "window-1");
    until_showing_something();
    let before = listed();
    daemon.call("tab.create", &json!({ "focus": false }));
    until(
        "this window to take the tab nobody asked for",
        || listed().len() == before.len() + 1,
        || format!("this window lists {:?} and the record says {:?}", listed(), holders(&daemon)),
    );
}

/// A tab this window asks for is its own, however recently another window was in front.
///
/// The failure measured on 0.8.1: the window that asked lost the tab to one that had not. herdr
/// describes a new tab to every window before the asking window hears the answer naming it, so
/// the rule for a tab nobody holds would hand it to the window in front - unless the asking window
/// has said it is waiting.
#[test]
fn a_tab_this_window_asks_for_stays_here_while_another_is_in_front() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let _other = another_window(&daemon, "window-9", i64::MAX / 2);
    open_a_window(&daemon, "window-1");
    let first = until_showing_something();

    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        take_focus: true,
        ..CreateTab::default()
    })));
    until(
        "the window to move onto the tab it asked for",
        || showing().is_some_and(|tab| tab != first),
        || format!("the window still shows {first}; the record says {:?}", holders(&daemon)),
    );
    let made = showing().expect("just waited for it");
    let record = holders(&daemon);
    assert!(
        record.iter().any(|(tab, window)| tab == &made && window == "window-1"),
        "the tab this window made is not recorded as its own: {record:?}"
    );
}

/// A pane this window moves into a tab of its own takes that tab with it, however recently another
/// window was in front.
///
/// herdr names the tab a move made somewhere other than where it names the tab `tab.create` made,
/// and reading only the second let the answer say nothing was made - so the window stopped waiting
/// empty-handed and the window in front took the tab. An agent in a background window pulling its
/// own pane out sent it to whatever window somebody was looking at.
#[test]
fn a_pane_this_window_moves_into_a_new_tab_stays_here_while_another_is_in_front() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let _other = another_window(&daemon, "window-9", i64::MAX / 2);
    open_a_window(&daemon, "window-1");
    let first = until_showing_something();
    daemon.call("pane.split", &json!({ "direction": "right" }));
    until(
        "the window to hear of the second pane",
        || panes_in_this_window().len() == 2,
        || format!("the window lists {:?}", panes_in_this_window()),
    );
    let moved = panes_in_this_window().pop().expect("just waited for two");

    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: moved.clone(),
        new_tab: true,
        ..ArrangePane::default()
    })));

    until(
        "the tab the move made to be recorded as this window's",
        || holders(&daemon).iter().any(|(tab, window)| tab != &first && window == "window-1"),
        || format!("the record says {:?}", holders(&daemon)),
    );
}

/// Going to a pane never shows a tab another window holds, even before this window has read that
/// it does.
///
/// The shell reads the record a moment after it moves, and a window still starting up has not read
/// it at all. A focus in that moment on a tab nobody held when this window last looked used to take
/// what it could, find the tab already taken, and show it anyway - taking its terminals from the
/// window that holds it.
#[test]
fn a_focus_never_shows_a_tab_the_record_gives_another_window() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let _other = another_window(&daemon, "window-9", 0);
    open_a_window(&daemon, "window-1");
    let first = until_showing_something();
    daemon.call("tab.create", &json!({ "focus": false }));
    // The pane as well as the tab: herdr can announce a tab before the pane in it.
    until(
        "this window, in front, to take the tab nobody asked for and hear of its pane",
        || listed().iter().any(|tab| tab != &first && !panes_in(tab).is_empty()),
        || format!("this window lists {:?}", listed()),
    );
    let theirs = listed().into_iter().find(|tab| tab != &first).expect("just waited for it");
    let pane = panes_in(&theirs).pop().expect("a new tab has a pane");

    // The other window comes to the front and the tab is held by nobody, which this window reads:
    // it lets the tab go and leaves it for the window in front.
    let path = record(&daemon);
    let mut holders = read_record(&path);
    holders.prune(|tab| tab.as_str() != theirs);
    holders.focused(&WindowName::new("window-9"), i64::MAX / 2);
    write_record(&path, &holders);
    assert_ok(&answer(request::Payload::ReadTabHolders(ReadTabHolders {})));
    assert_eq!(listed(), vec![first.clone()], "this window kept a tab it was not due");

    // Then the other window takes it, and this window has not read that yet.
    give(&daemon, &theirs, "window-9");
    let focused = answer(request::Payload::FocusPane(FocusPane {
        pane_id: pane.clone(),
        ..FocusPane::default()
    }));

    assert!(
        matches!(focused.payload, Some(response::Payload::Failure(_))),
        "a focus on {pane} in another window's tab was carried out here: {focused:?}"
    );
    assert_eq!(showing(), Some(first), "this window showed a tab another window holds");
}

/// A tab another window takes leaves this one, and the panes in it keep running.
#[test]
fn a_tab_another_window_takes_leaves_this_one() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let _other = another_window(&daemon, "window-9", 0);
    open_a_window(&daemon, "window-1");
    let first = until_showing_something();
    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        take_focus: true,
        ..CreateTab::default()
    })));
    until(
        "the second tab to arrive",
        || listed().len() == 2,
        || format!("this window lists {:?}", listed()),
    );

    give(&daemon, &first, "window-9");
    assert_ok(&answer(request::Payload::ReadTabHolders(ReadTabHolders {})));

    assert!(
        !listed().contains(&first),
        "{first} was given to another window and is still listed here: {:?}",
        listed()
    );
    let tabs = daemon.call("session.snapshot", &json!({}))["snapshot"]["tabs"].clone();
    assert_eq!(
        tabs.as_array().map_or(0, Vec::len),
        2,
        "moving a tab between windows closed something on the daemon: {tabs}"
    );
}

/// A window somebody closed keeps its tabs, and a window opened beside it does not take them.
#[test]
fn a_closed_window_keeps_its_tabs() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
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
    let daemon = Daemon::start();
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
/// to be safe. Read as empty, it said this window held nothing, and the window let go of every
/// tab with nothing to bring them back short of a relaunch.
#[test]
fn a_record_deleted_under_an_open_window_is_written_again() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    open_a_window(&daemon, "window-1");
    let ours = until_showing_something();

    std::fs::remove_file(record(&daemon)).expect("the window wrote a record");
    assert_ok(&answer(request::Payload::ReadTabHolders(ReadTabHolders {})));

    assert_eq!(listed(), vec![ours.clone()], "the window let go of its tab when the record went");
    assert_eq!(
        holders(&daemon),
        vec![(ours, "window-1".to_string())],
        "the window did not write itself and its tab back into the record"
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
    let daemon = Daemon::start();
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
    let daemon = Daemon::start();
    tabs_already_running(&daemon);
    start_a_window(&daemon, "window-1");
    let pane = a_named_pane(&daemon);

    assert!(matches!(
        answer(request::Payload::AttachPane(AttachPane { pane_id: pane })).payload,
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
    let daemon = Daemon::start();
    open_a_window(&daemon, "window-1");
    until_showing_something();
    assert_ok(&answer(request::Payload::Quitting(muster::proto::Quitting::default())));
    daemon.call("tab.create", &json!({ "focus": false }));

    turn.relaunch();
    open_a_window(&daemon, "window-1");

    until(
        "the reopened window to list the tab made while it was closed",
        || listed().len() == 2,
        || format!("this window lists {:?}; the record says {:?}", listed(), holders(&daemon)),
    );
}

/// A tab this window left for the window in front is taken once this window comes to the front.
///
/// Coming to the front is what makes a window the one a tab nobody holds joins, so it has to ask
/// again then, rather than wait for the daemon to say something else.
#[test]
fn coming_to_the_front_takes_the_tabs_left_for_the_window_that_was() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let _other = another_window(&daemon, "window-9", i64::MAX / 2);
    open_a_window(&daemon, "window-1");
    until_showing_something();
    daemon.call("tab.create", &json!({ "focus": false }));
    until(
        "this window to hear of the tab nobody asked for",
        || panes_on_the_daemon() == 2,
        || format!("the window has heard of {} panes", panes_on_the_daemon()),
    );
    assert_eq!(listed().len(), 1, "this window took a tab the window in front was due");
    // The other window goes behind, as it would once this one is in front.
    let path = record(&daemon);
    let mut holders = read_record(&path);
    holders.focused(&WindowName::new("window-9"), 0);
    write_record(&path, &holders);

    assert_ok(&answer(request::Payload::WindowFocus(WindowFocus { focused: true })));

    assert_eq!(listed().len(), 2, "coming to the front did not take the tab nobody holds");
}

/// Stands in for a window that is open: a socket that answers, named in the shared record.
fn another_window(daemon: &Daemon, name: &str, focused: i64) -> UnixListener {
    let socket = daemon.root().join(format!("{name}.sock"));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).expect("a socket can be bound in the scratch root");
    let arrangement = daemon.root().join(format!("{name}.toml"));
    std::fs::write(&arrangement, "").expect("a stand-in arrangement can be written");
    let path = record(daemon);
    let mut holders = read_record(&path);
    holders.opened(HeldWindow {
        name: WindowName::new(name),
        arrangement: arrangement.to_string_lossy().into_owned(),
        socket: socket.to_string_lossy().into_owned(),
        pid: 1,
        focused,
        daemons: std::iter::once(DaemonId::new("local")).collect(),
    });
    write_record(&path, &holders);
    listener
}

/// Gives a tab to another window, as that window would.
fn give(daemon: &Daemon, tab: &str, window: &str) {
    let path = record(daemon);
    let mut holders = read_record(&path);
    holders.take(TabId::new(tab), &WindowName::new(window));
    write_record(&path, &holders);
}

/// Every tab the record names, with the window holding it.
fn holders(daemon: &Daemon) -> Vec<(String, String)> {
    let holders = read_record(&record(daemon));
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
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
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
        pane_names_path: daemon.root().join("panes.toml").to_string_lossy().into_owned(),
        tab_holders_path: record(daemon).to_string_lossy().into_owned(),
        ..Startup::default()
    })));
}

/// Two tabs on the daemon before any window opens: one split three ways, and one of a single pane.
///
/// What `tools/smoke-launch.py` stages, and what Alex's machine looks like at his first launch of
/// the release that brought this record: every tab already there, and no record saying whose.
fn tabs_already_running(daemon: &Daemon) {
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "work", "focus": true }));
    daemon.call("pane.split", &json!({ "direction": "right" }));
    daemon.call("pane.split", &json!({ "direction": "down" }));
    daemon.call("tab.create", &json!({ "focus": false }));
}

/// Muster's name for a pane the daemon already held, once the window has named it.
fn a_named_pane(daemon: &Daemon) -> String {
    let path = daemon.root().join("panes.toml");
    let first = || -> Option<String> {
        let text = std::fs::read_to_string(&path).ok()?;
        let (panes, _) =
            muster_core::names::from_toml(&text, muster_core::names::Mint::Drawn).ok()?;
        panes.entries().next().map(|(name, _, _)| name.to_string())
    };
    until(
        "the window to name the panes the daemon holds",
        || first().is_some(),
        || format!("{} names nothing yet", path.display()),
    );
    first().expect("just waited for it")
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

/// How many panes the daemon holds, as far as this window has heard.
fn panes_on_the_daemon() -> u32 {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window.daemons.iter().map(|d| d.panes).sum(),
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

/// The panes in the tabs this window lists.
fn panes_in_this_window() -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window
            .roster
            .iter()
            .flat_map(|roster| roster.tabs.iter())
            .flat_map(|tab| tab.panes.iter())
            .map(|pane| pane.pane_id.clone())
            .collect(),
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

/// The panes in one tab this window lists.
fn panes_in(tab: &str) -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window
            .roster
            .iter()
            .flat_map(|roster| roster.tabs.iter())
            .filter(|held| held.tab_id == tab)
            .flat_map(|held| held.panes.iter())
            .map(|pane| pane.pane_id.clone())
            .collect(),
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

/// The tabs this window lists, in its order.
fn listed() -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
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
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Made(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
