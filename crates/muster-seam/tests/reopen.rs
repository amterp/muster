//! A window coming back the way it was left, against a real daemon.
//!
//! The arrangement is the one thing Muster owns and no daemon can answer: which of its tabs
//! this window was showing, in what order, at what widths. `composition-saved.json` pins the
//! rules for reading a saved arrangement back; what needs a daemon is the round trip - that a
//! window settles, writes, and that a second one opening onto the same session shows the same
//! thing.
//!
//! Two processes cannot be had here, because the seam holds one session per process. So the
//! file is the seam: this drives a window, reads what it wrote, and asserts on that - which is
//! also the only part a second process would have to go on. `Turn::relaunch` then takes the
//! other half of the round trip, opening a second window onto the file the first one left, in
//! the same test.
//!
//! The window somebody asks for is here too, because it is the same subject from the other
//! side: what a launch does about the arrangement the one before it left. A window Muster comes
//! back to takes it; a window somebody asked for takes none of it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use herdr_harness::{Daemon, until};
use muster::proto::{
    CreateTab, Event, OpenWindow, ReadWindow, Request, Response, SplitPane, Startup, ViewChanged,
    ViewNode, ViewRegion, event, request, response, view_node,
};
use muster_core::mirror::backend::TabId;
use prost::Message;

#[test]
fn a_window_writes_down_what_it_is_showing() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let state = daemon.muster_config().with_file_name("window.toml");

    // The statics below outlive a test, so what the last one published is still there and
    // would answer this one's first wait instantly.
    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));
    open_a_window(&daemon, &state);

    until(
        "the window to open onto a workspace",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let first = tab_of_first_region().expect("just waited for it");

    // Somewhere to move to, so what is written down is a choice rather than the only thing
    // there was. A new tab is what a person reaches for several times an hour.
    assert_ok(&answer(request::Payload::CreateTab(CreateTab::default())));
    until(
        "the window to move onto the tab it asked for",
        || tab_of_first_region().is_some_and(|tab| tab != first),
        || format!("the region still shows {first}"),
    );
    let second = tab_of_first_region().expect("just waited for it");

    let written = std::fs::read_to_string(&state)
        .unwrap_or_else(|e| panic!("the window wrote no arrangement to {}: {e}", state.display()));

    // Both tabs, because the window holds both - and `showing` naming the one it ended on. A
    // file that said the first was on screen would be one written at open and never again,
    // which is the failure that looks most like working.
    assert!(
        written.contains(&format!("showing = \"{second}\"")),
        "the arrangement does not say the window was showing {second}:\n{written}"
    );
    assert!(
        written.contains(&first),
        "the arrangement dropped the tab the window moved off ({first}), which it still holds \
         and can switch back to:\n{written}"
    );
    // Intent, not observation. The tab somebody chose is written; how that tab was arranged
    // is not, because it is the daemon's to say and it will have moved on. A file carrying a
    // pane tree is one that can disagree with the session it reopens onto.
    for daemon_truth in ["columns", "rows", "ratio", "split"] {
        assert!(
            !written.contains(daemon_truth),
            "the arrangement wrote down {daemon_truth:?}, which is the daemon's answer and \
             not this window's:\n{written}"
        );
    }
    // Equal widths are a default nobody minds losing on restart; a width somebody dragged is
    // not, which is what turned this from tidiness into something users notice.
    assert!(written.contains("weight"), "no width was written down at all:\n{written}");

    // And the whole point of writing it: a window opening onto this session again shows the
    // tab it was left on. Read through the same rules the core uses, against the tabs this
    // daemon actually holds - which is the check that makes a saved region a wish.
    let saved = muster_core::composition::saved::from_toml(&written)
        .expect("the core can read back what it just wrote");
    let restorable = saved.restorable(|_, tab| tab.as_str() == second);
    assert_eq!(
        restorable.tabs.len(),
        1,
        "reopening onto this session holds {} tabs rather than the one this daemon still has",
        restorable.tabs.len()
    );
    assert_eq!(restorable.tabs[0].id.as_str(), second);
    assert_eq!(restorable.showing.as_ref().map(TabId::as_str), Some(second.as_str()));
}

/// A window opening onto a saved arrangement shows each of its tabs once.
///
/// The failure this pins produced a window drawing the same pane in two regions, which is
/// two surfaces, two bridges, and a second one refused the terminal - a dead panel that
/// cannot be closed from inside Muster, because closing it would close the pane the live
/// region beside it is using.
///
/// It took two things at once, which is why it needs a relaunch and the order below. A
/// window with no saved arrangement never showed it, and neither did a region opened during
/// a session: the second region came from the restore path, on top of one the daemon's first
/// bootstrap had already opened while the window was still being built.
#[test]
fn a_window_reopens_each_saved_tab_once() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let state = daemon.muster_config().with_file_name("window.toml");

    // The statics below outlive a test, so what the last one published is still there and
    // would answer this one's first wait instantly.
    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));
    open_a_window(&daemon, &state);
    until(
        "the first window to open onto a workspace",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let showing = tab_of_first_region().expect("just waited for it");
    assert_eq!(regions_shown(), 1, "the first window opened more than one region");

    // Everything the second launch gets, and everything a second process would get: the file
    // the first one wrote on its way out.
    turn.relaunch();
    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));

    // Following, and then a wait, and only then opening. That is the app's own order - it
    // starts the core, builds a renderer, a menu and a window, and asks for the window to be
    // opened last - and the gap is where this bug lived: the daemon's first bootstrap lands
    // inside it and gives the daemon a region of its own. Dispatching both requests
    // back to back would close the gap and test nothing.
    assert_ok(&answer(request::Payload::Startup(startup(&daemon, &state))));
    until("the daemon's first bootstrap to be taken", bootstrapped, || {
        format!("the last view the core published: {:?}", latest_view())
    });
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    assert_eq!(
        regions_shown(),
        1,
        "the second window drew {} regions for one saved arrangement, so at least one pane \
         in it is shown twice",
        regions_shown()
    );
    assert_eq!(tab_of_first_region().as_deref(), Some(showing.as_str()));

    // And what it writes down says the same, which is what stops a window that has done this
    // once from doing it worse every launch after.
    let written = std::fs::read_to_string(&state)
        .unwrap_or_else(|e| panic!("the second window wrote no arrangement: {e}"));
    assert_eq!(
        written.matches("[[region]]").count(),
        1,
        "the saved arrangement grew a region across a relaunch:\n{written}"
    );
}

/// An arrangement naming one tab twice still opens it once.
///
/// The other half of the same bug, and the half that decides whether anybody gets their
/// window back. A Muster that drew a pane twice wrote two regions for it on the way out, so
/// there are files on real machines holding the same tab two and three times - and a fix that
/// only stopped new ones being written would leave those windows broken every launch, with
/// deleting the file the only way out.
///
/// The file here is one this build wrote, doubled, rather than one composed by hand: a
/// fixture that guessed at the format would keep passing after the format moved.
#[test]
fn an_arrangement_naming_one_tab_twice_opens_it_once() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let state = daemon.muster_config().with_file_name("window.toml");

    // The statics below outlive a test, so what the last one published is still there and
    // would answer this one's first wait instantly.
    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));
    open_a_window(&daemon, &state);
    until(
        "the first window to open onto a workspace",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let showing = tab_of_first_region().expect("just waited for it");

    turn.relaunch();
    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));

    let written = std::fs::read_to_string(&state).expect("the first window wrote an arrangement");
    let (head, region) = written
        .split_once("[[region]]")
        .expect("the arrangement the first window wrote holds a region");
    std::fs::write(&state, format!("{head}[[region]]{region}\n[[region]]{region}"))
        .expect("the harness root is writable");

    open_a_window(&daemon, &state);
    until(
        "the second window to open onto the tab it was left on",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    assert_eq!(
        regions_shown(),
        1,
        "the window opened {} regions for a file naming one tab twice, so the pane in it is \
         drawn twice and one of the two cannot attach",
        regions_shown()
    );
    assert_eq!(tab_of_first_region().as_deref(), Some(showing.as_str()));

    // And the file is left holding one, so the window heals rather than staying one launch
    // away from doing this again.
    let after = std::fs::read_to_string(&state).expect("the second window wrote an arrangement");
    assert_eq!(
        after.matches("[[region]]").count(),
        1,
        "the duplicate survived into what this window wrote:\n{after}"
    );
}

/// A window somebody asked for opens onto a tab of its own.
///
/// Two windows on one machine is the arrangement this is about, and the daemon between them is
/// what makes it sharp: herdr allows one client per terminal, so a second window that opened
/// onto the tab the first one is showing gets every attach refused and renders four dead
/// surfaces. Measured that way on 0.4.1 (kan `a_2IZ5TL6DQ`) - six panes, four bridges, four
/// `already has an attached client`.
///
/// Nothing here can see the first window's bridges, because a process holds one session. What
/// it can see is the thing that decides the outcome: which tab the second window opens onto.
/// A tab the first window was not on is a tab whose terminals are free.
///
/// The daemon holding a tab already is the whole of the setup. A launch with no saved
/// arrangement is not enough on its own - the standing rule gives every machine a region on
/// whatever tab that machine last had focused, and that is exactly the tab the window before it
/// was showing.
#[test]
fn a_window_somebody_asked_for_opens_onto_a_tab_of_its_own() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let state = daemon.muster_config().with_file_name("window.toml");

    // The statics below outlive a test, so what the last one published is still there and
    // would answer this one's first wait instantly.
    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));
    open_a_window(&daemon, &state);
    until(
        "the first window to open onto a workspace",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let theirs = tab_of_first_region().expect("just waited for it");

    turn.relaunch();
    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));

    // What `muster window new` sends: no arrangement to remember, and a launch that says
    // somebody asked for it. The daemon is the same one, still holding the tab above, which is
    // what the first window would still be showing.
    assert_ok(&answer(request::Payload::Startup(Startup {
        fresh: true,
        state_path: String::new(),
        ..startup(&daemon, &state)
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until(
        "the window somebody asked for to open onto something",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let ours = tab_of_first_region().expect("just waited for it");
    assert_ne!(
        ours, theirs,
        "the window opened onto the tab the last one was showing ({theirs}), so on a machine \
         where that window is still open every pane in it is a surface herdr will refuse"
    );

    // And it remembers nothing, which is the other half: two windows writing one file means the
    // arrangement that comes back is whichever of them published last.
    let written = std::fs::read_to_string(&state).expect("the first window wrote an arrangement");
    assert!(
        written.contains(&theirs) && !written.contains(&ours),
        "the window somebody asked for wrote over the arrangement the other one left:\n{written}"
    );
}

/// A window Muster comes back to still takes the tabs it was left on.
///
/// The other side of the rule above, and the reason it is a flag rather than a change of
/// behaviour: everything about a Dock launch stays as it was, including the case that matters
/// most - coming back to a session full of agents that were running before Muster quit.
#[test]
fn a_window_muster_comes_back_to_still_takes_what_it_was_left() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let state = daemon.muster_config().with_file_name("window.toml");

    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));
    open_a_window(&daemon, &state);
    until(
        "the first window to open onto a workspace",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let showing = tab_of_first_region().expect("just waited for it");

    turn.relaunch();
    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));
    open_a_window(&daemon, &state);
    until(
        "the window to come back onto the tab it was left on",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    assert_eq!(
        tab_of_first_region().as_deref(),
        Some(showing.as_str()),
        "a launch with an arrangement to come back to opened somewhere else"
    );
}

/// A saved region naming a pane the daemon has dropped puts the keyboard on one it holds.
///
/// Pinning what is already true rather than fixing anything, and it is worth pinning because
/// the obvious reading of the failure in `a_2IZ5TL6DQ` was that a saved pane reached a bridge.
/// It cannot: a bridge follows the tab's published tree and every pane in it is one the mirror
/// holds, so what a saved region names is only ever the keyboard's place. This says the
/// keyboard lands somewhere real, which is the part that would be a window ignoring keystrokes
/// with nothing on screen to say why.
#[test]
fn a_saved_region_naming_a_dropped_pane_puts_the_keyboard_on_one_that_is_there() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let state = daemon.muster_config().with_file_name("window.toml");

    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));
    open_a_window(&daemon, &state);
    until(
        "the first window to open onto a workspace",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // A second pane, so the tab survives losing one - the window's own split, so the keyboard
    // follows it and the arrangement written down names it.
    assert_ok(&answer(request::Payload::SplitPane(SplitPane {
        side: "down".to_string(),
        ..SplitPane::default()
    })));
    until(
        "the split to reach the window",
        || panes_on_screen() == 2,
        || format!("the last view the core published: {:?}", latest_view()),
    );

    turn.relaunch();
    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));

    // The pane the arrangement names is gone before the window opens, which is the state a
    // relaunch meets whenever something finished while Muster was not running.
    let held = daemon_panes(&daemon);
    let dropped = held.last().expect("the daemon holds the panes this window made");
    daemon.call("pane.close", &serde_json::json!({ "pane_id": dropped }));

    open_a_window(&daemon, &state);
    until(
        "the window to come back onto what is left",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    let view = latest_view().expect("just waited for it");
    let region = view.regions.first().expect("the window opened a region");
    let drawn = panes_of_region(region);
    assert!(
        drawn.contains(&region.pane_id),
        "the keyboard is on {} and the region is drawing {drawn:?}, so every keystroke goes \
         to a pane that is not there",
        region.pane_id
    );
}

/// A restored window says which panes it is drawing.
///
/// `on_screen` is what an agent driving Muster reads to decide whether a pane needs surfacing,
/// and what the sidebar's shared-screen mark is built on - so a pane wrongly marked hidden
/// invites something to go and surface a pane already in front of the user. Reported on 0.4.1
/// against a restored four-pane tab, where three of the four read hidden while all four were
/// being drawn (kan `a_2Ibz6NXjV`).
///
/// Asked through `ReadWindow`, which is the surface the report came from, rather than off the
/// view events: what went wrong is a disagreement between the two halves of one answer, and
/// only the answer holds both.
#[test]
fn a_restored_window_says_which_panes_it_is_drawing() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let state = daemon.muster_config().with_file_name("window.toml");

    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));
    open_a_window(&daemon, &state);
    until(
        "the first window to open onto a workspace",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // Four panes in one tab, in a real split rather than a stack: the derivation only goes
    // wrong where a region shows more than one pane, so three panes would already be enough
    // and a fourth costs nothing and matches what was reported.
    for side in ["down", "right", "down"] {
        assert_ok(&answer(request::Payload::SplitPane(SplitPane {
            side: side.to_string(),
            ..SplitPane::default()
        })));
    }
    until(
        "the window to draw all four panes",
        || panes_on_screen() == 4,
        || format!("the last view the core published: {:?}", latest_view()),
    );

    turn.relaunch();
    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));
    open_a_window(&daemon, &state);
    until(
        "the window to come back onto its four panes",
        || panes_on_screen() == 4,
        || format!("the last view the core published: {:?}", latest_view()),
    );

    let window = read_window();
    let drawn = window
        .view
        .as_ref()
        .map(|view| view.regions.iter().flat_map(panes_of_region).collect::<Vec<_>>())
        .unwrap_or_default();
    let hidden: Vec<String> = window
        .roster
        .iter()
        .flat_map(|roster| roster.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .filter(|pane| !pane.on_screen && drawn.contains(&pane.pane_id))
        .map(|pane| pane.pane_id.clone())
        .collect();

    assert!(
        hidden.is_empty(),
        "the window is drawing {drawn:?} and reports {hidden:?} as hidden. Anything reading \
         this to decide whether a pane needs surfacing will go and surface a pane that is \
         already in front of the user."
    );
}

/// What `muster window` would print at this moment.
fn read_window() -> muster::proto::Window {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

/// Starts the core against this daemon, and opens the window - what a bare `muster` does.
fn open_a_window(daemon: &Daemon, state: &std::path::Path) {
    assert_ok(&answer(request::Payload::Startup(startup(daemon, state))));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
}

fn startup(daemon: &Daemon, state: &std::path::Path) -> Startup {
    Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: state.to_string_lossy().into_owned(),
        // Named, because a saved region says which tab it was showing in Muster's own name for
        // it. A relaunch that minted fresh names would fail every region's check and open as a
        // first launch - which is the app's own behaviour without this file, and would leave a
        // relaunch here testing nothing.
        pane_names_path: daemon.root().join("panes.toml").to_string_lossy().into_owned(),
        ..Startup::default()
    }
}

fn regions_shown() -> usize {
    latest_view().map_or(0, |view| view.regions.len())
}

/// Every pane one region is drawing, in no particular order.
fn panes_of_region(region: &ViewRegion) -> Vec<String> {
    let mut found = Vec::new();
    if let Some(root) = &region.root {
        collect(root, &mut found);
    }
    found
}

fn collect(node: &ViewNode, found: &mut Vec<String>) {
    match &node.node {
        Some(view_node::Node::Pane(pane)) => found.push(pane.pane_id.clone()),
        Some(view_node::Node::Split(split)) => {
            for child in [split.first.as_deref(), split.second.as_deref()].into_iter().flatten() {
                collect(child, found);
            }
        }
        None => {}
    }
}

fn panes_on_screen() -> usize {
    latest_view()
        .map(|view| view.regions.iter().map(|region| panes_of_region(region).len()).sum())
        .unwrap_or_default()
}

/// Every pane the daemon holds, in the order it lists them.
fn daemon_panes(daemon: &Daemon) -> Vec<String> {
    let snapshot = daemon.call("session.snapshot", &serde_json::json!({}));
    snapshot["snapshot"]["panes"]
        .as_array()
        .map(|panes| {
            panes.iter().filter_map(|pane| pane["pane_id"].as_str().map(str::to_string)).collect()
        })
        .unwrap_or_default()
}

fn tab_of_first_region() -> Option<String> {
    let view = latest_view()?;
    let region = view.regions.first()?;
    // A region with no tree is one the daemon has not described yet, which is not the state
    // this is waiting for.
    region.root.as_ref()?;
    Some(region.tab_id.clone())
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);

/// Whether a daemon has reported itself connected, which is the end of a bootstrap.
///
/// The one moment in a launch a test can wait for that is not about what is on screen. The
/// subscription's first bootstrap reconciles, publishes, and only then says this - so a
/// window that has seen it has had everything the daemon's arrival was going to do to it,
/// including nothing.
static CONNECTED: AtomicBool = AtomicBool::new(false);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::ViewChanged(view)) => {
            *VIEW.lock().expect("a panicking test poisoned the view") = Some(view);
        }
        Some(event::Payload::BackendHealth(health)) if health.state == "connected" => {
            CONNECTED.store(true, Ordering::Relaxed);
        }
        _ => {}
    }
}

fn latest_view() -> Option<ViewChanged> {
    VIEW.lock().expect("a panicking test poisoned the view").clone()
}

fn bootstrapped() -> bool {
    CONNECTED.load(Ordering::Relaxed)
}

/// Throws away what the last launch said, so a wait on the next one cannot be answered by the
/// window before it.
fn forget_the_view() {
    *VIEW.lock().expect("a panicking test poisoned the view") = None;
    CONNECTED.store(false, Ordering::Relaxed);
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

/// That the core accepted a request, whichever shape its acceptance took.
///
/// `Made` is an acceptance too: a request that creates a pane answers with the pane rather than
/// with a bare Ok, because the name was minted inside the call and a caller cannot learn it any
/// other way. Only `Failure` is a refusal.
fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Made(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
