//! What ⌘1 to ⌘9 mean, end to end against a real daemon.
//!
//! Every layer of this is already covered: `bindings.json` pins the nine chords, `roster.json`
//! pins the numbering they name, and `composition.json` pins what surfacing a tab does. What
//! is not covered anywhere else is the composition of those layers - which is the shape every
//! bug this project has shipped recently has had, each one green at every level and wrong when
//! the app ran.
//!
//! So this asserts the gesture: the number the roster hands the shell is the number the shell
//! can send back, and doing so lands the keyboard on the pane whose row carries it. The
//! interesting case is a pane in a tab nothing is showing, because that is the argument for
//! numbering panes at all - reaching one has to bring its tab on screen, or nine chords do not
//! replace what the tab numbers used to do.
//!
//! The second test is the prototype scheme beside it, for the same reason: every layer of a
//! two-stage chord is pinned in the corpus, and what only a running window shows is that the
//! first press does not quietly disarm itself on whatever it causes the shell to send back.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use herdr_harness::{Daemon, until};
use muster::proto::{
    EndNumberedChord, Event, FocusPaneAt, OpenWindow, ReloadConfig, Request, Response,
    RosterChanged, Startup, ViewChanged, WindowFocus, event, request, response,
    roster_changed::Counting,
};
use prost::Message;
use serde_json::{Value, json};

#[test]
fn a_numbered_chord_lands_on_the_row_carrying_that_number() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start();
    a_session_of_two_tabs(&daemon);

    // Registered before startup, because startup begins following the configured daemons and a
    // callback added afterwards misses the first bootstrap entirely.
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until(
        "the roster to arrive with both tabs in it",
        || roster().is_some_and(|roster| places(&roster).len() == 2),
        || format!("the roster holds {:?}", roster().map(|r| places(&r))),
    );

    // The numbering is one count across every tab, so the pane in the second tab is 2 even
    // though it is the first pane of its own tab. Read off the roster rather than assumed: the
    // whole point is that the shell sends back the number it was given.
    let numbered = places(&roster().expect("just waited for it"));
    assert_eq!(
        numbered,
        vec![(1, VISIBLE.to_string()), (2, HIDDEN.to_string())],
        "the count should run across tabs, so the second tab's first pane is 2"
    );
    let hidden = named(HIDDEN);

    // The tab holding pane 2 is not on screen: one region, showing the first tab. That is the
    // case the numbers exist for, and the one a tab-numbering scheme handled by numbering tabs.
    assert!(
        !showing(&hidden),
        "this test is pointless unless pane 2 starts hidden, and the view already shows it"
    );

    assert_ok(&answer(request::Payload::FocusPaneAt(FocusPaneAt { place: 2 })));

    until(
        "the hidden pane to be on screen with the keyboard",
        || showing(&hidden),
        || format!("the view still shows {:?}", shown()),
    );

    // Surfaced rather than opened beside: a second region onto the same daemon would be two
    // copies of one window, and the region count is what tells those apart.
    assert_eq!(regions(), 1, "reaching a hidden pane opened a region instead of retargeting one");

    // And the refusal, in the same breath, because a place past the end is what ⌘9 means in a
    // window of two and it has to do nothing rather than land somewhere.
    let reason = refusal(request::Payload::FocusPaneAt(FocusPaneAt { place: 9 }));
    assert!(
        reason.contains("2 panes") && reason.contains("no pane 9"),
        "a place past the end should say how many there are, and said: {reason}"
    );
    assert!(
        showing(&hidden),
        "a refused chord moved the keyboard, so it did something rather than nothing"
    );
}

#[test]
fn under_the_prototype_a_tab_is_named_first_and_a_pane_inside_it_second() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start();
    a_session_of_two_tabs_the_second_holding_two(&daemon);

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon
            .muster_config_with("numbered_chords = \"tab_then_pane\"")
            .to_string_lossy()
            .into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until(
        "the roster to arrive with all three panes in it",
        || roster().is_some_and(|roster| rows(&roster).len() == 3),
        || format!("the roster holds {:?}", roster().map(|held| places(&held))),
    );

    // At rest the numbers are on the tabs and nowhere else, which is the whole indicator: what
    // ⌘2 does is answered by reading the list rather than by remembering what you last pressed.
    assert_eq!(numbered_tabs(), vec![1, 2], "the chords should be naming tabs before any press");
    assert_eq!(
        numbered_panes(),
        Vec::<u32>::new(),
        "a pane carrying a number while the tabs do would be two numberings in one list"
    );

    let inner_second = named(INNER_SECOND);
    assert_ok(&answer(request::Payload::FocusPaneAt(FocusPaneAt { place: 2 })));

    // Acted on immediately rather than waiting for a second press: the tab is on screen and the
    // keyboard is on its first pane, which is where a click on its caption would have put it.
    until(
        "the second tab to be on screen with the keyboard on its first pane",
        || showing(&named(INNER_FIRST)),
        || format!("the view still shows {:?}", shown()),
    );

    // And the numbers have moved inside it, so the second press is legible before it is made.
    assert_eq!(
        numbered_tabs(),
        Vec::<u32>::new(),
        "the tabs kept their numbers after one was named, so two things are numbered at once"
    );
    assert_eq!(
        numbered_panes(),
        vec![1, 2],
        "the named tab's panes should be the numbered ones, and only them"
    );

    assert_ok(&answer(request::Payload::FocusPaneAt(FocusPaneAt { place: 2 })));
    until(
        "the second pane of the second tab to have the keyboard",
        || showing(&inner_second),
        || format!("the view still shows {:?}", shown()),
    );

    // The flat scheme would have landed the same two presses on the second pane of the window
    // twice over, which is a different pane - so this passing under both schemes is impossible.
    assert_ne!(inner_second, named(INNER_FIRST), "the arrangement this test needs came apart");
}

#[test]
fn anything_between_the_two_presses_takes_the_first_one_back() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start();
    a_session_of_two_tabs_the_second_holding_two(&daemon);

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon
            .muster_config_with("numbered_chords = \"tab_then_pane\"")
            .to_string_lossy()
            .into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the roster to arrive with all three panes in it",
        || roster().is_some_and(|roster| rows(&roster).len() == 3),
        || format!("the roster holds {:?}", roster().map(|held| places(&held))),
    );

    assert_ok(&answer(request::Payload::FocusPaneAt(FocusPaneAt { place: 2 })));
    until(
        "the second tab to be named",
        || numbered_panes() == vec![1, 2],
        || format!("the tabs carry {:?} and the panes {:?}", numbered_tabs(), numbered_panes()),
    );

    // The window losing focus is one of the ordinary things that happen between two keystrokes,
    // and it stands here for all of them: the rule is that anything which is not a read takes
    // the arm back. Asserted through the published numbers rather than through where the next
    // press lands, because the numbers are what a person is reading while deciding to press.
    assert_ok(&answer(request::Payload::WindowFocus(WindowFocus { focused: false })));
    until(
        "the numbers to go back to the tabs",
        || numbered_tabs() == vec![1, 2],
        || format!("the tabs carry {:?} and the panes {:?}", numbered_tabs(), numbered_panes()),
    );

    // So the press that follows is a first press again, and reaches a tab rather than a pane.
    assert_ok(&answer(request::Payload::FocusPaneAt(FocusPaneAt { place: 1 })));
    until(
        "the first tab's only pane to have the keyboard",
        || showing(&named(VISIBLE)),
        || format!("the view still shows {:?}", shown()),
    );
}

#[test]
fn letting_go_of_the_modifier_takes_the_first_press_back() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start();
    a_session_of_two_tabs_the_second_holding_two(&daemon);

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon
            .muster_config_with("numbered_chords = \"tab_then_pane\"")
            .to_string_lossy()
            .into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the roster to arrive with all three panes in it",
        || roster().is_some_and(|roster| rows(&roster).len() == 3),
        || format!("the roster holds {:?}", roster().map(|held| places(&held))),
    );
    assert_eq!(counting(), Counting::Tabs, "the chords should be naming tabs before any press");

    assert_ok(&answer(request::Payload::FocusPaneAt(FocusPaneAt { place: 2 })));
    until(
        "the second tab to be named",
        || counting() == Counting::PanesInTab,
        || format!("the roster says the chords are counting {:?}", counting()),
    );
    // What releasing ⌘ means. Distinct from every other way a chord ends, because it is the one
    // that happens when somebody decides mid-gesture that the tab was all they wanted - and
    // until this existed, walking away from the keyboard there left the numbers waiting.
    assert_ok(&answer(request::Payload::EndNumberedChord(EndNumberedChord {})));
    until(
        "the numbers to go back to the tabs",
        || numbered_tabs() == vec![1, 2],
        || format!("the tabs carry {:?} and the panes {:?}", numbered_tabs(), numbered_panes()),
    );
    assert_eq!(counting(), Counting::Tabs, "the roster still says a chord is half-typed");

    // So the press after it is a first press again, and reaches a tab rather than a pane. This
    // is the whole complaint the change answers: ⌘2, let go, ⌘1 should be two tab jumps.
    assert_ok(&answer(request::Payload::FocusPaneAt(FocusPaneAt { place: 1 })));
    until(
        "the first tab's only pane to have the keyboard",
        || showing(&named(VISIBLE)),
        || format!("the view still shows {:?}", shown()),
    );
}

#[test]
fn ending_a_chord_nobody_started_says_nothing() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start();
    a_session_of_two_tabs_the_second_holding_two(&daemon);

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon
            .muster_config_with("numbered_chords = \"tab_then_pane\"")
            .to_string_lossy()
            .into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the roster to arrive with all three panes in it",
        || roster().is_some_and(|roster| rows(&roster).len() == 3),
        || format!("the roster holds {:?}", roster().map(|held| places(&held))),
    );

    // The shell only sends this while a chord is half-typed, but it decides that from a roster
    // that is by then a moment old, so the harmless case has to stay harmless. Counted rather
    // than eyeballed: an agent list that repainted on every ⌘ release would repaint on ⌘C.
    //
    // Settled first, because this is the one assertion here about something *not* happening -
    // and a daemon still finishing its bootstrap would publish under it and read as a failure
    // in code that had done nothing wrong.
    let before = once_quiet();
    assert_ok(&answer(request::Payload::EndNumberedChord(EndNumberedChord {})));
    assert_eq!(
        rosters(),
        before,
        "ending a chord nobody started republished the roster, so every ⌘ release would redraw \
         the agent list"
    );
}

#[test]
fn turning_the_prototype_off_moves_the_numbers_back_on_the_save() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start();
    a_session_of_two_tabs_the_second_holding_two(&daemon);

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon
            .muster_config_with("numbered_chords = \"tab_then_pane\"")
            .to_string_lossy()
            .into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the chords to be naming tabs",
        || numbered_tabs() == vec![1, 2],
        || format!("the tabs carry {:?} and the panes {:?}", numbered_tabs(), numbered_panes()),
    );

    // Written to the same path, which is what saving the file is. Going back to the settled
    // scheme is the likeliest thing to happen to this option, so it is the half worth pinning.
    daemon.muster_config_with("numbered_chords = \"panes\"");
    assert_ok(&answer(request::Payload::ReloadConfig(ReloadConfig {})));

    // Asserted the moment the reload returns rather than waited for, and that is the test.
    // A reload asks the daemon to re-read its own config, so it says something shortly
    // afterwards and the roster is republished anyway - which means a version of this that
    // waited would pass whether or not the reload announced anything, and would be pinning the
    // daemon's timing rather than Muster's guarantee.
    assert_eq!(
        numbered_panes(),
        vec![1, 2, 3],
        "the save returned with the numbers still on the tabs, so the sidebar was promising \
         that ⌘2 reaches the second tab while it had already gone back to the second pane"
    );
    assert_eq!(numbered_tabs(), Vec::<u32>::new(), "the tabs kept numbers the chords do not name");

    // And the chords agree with them: ⌘2 is the second pane of the window again, in one press.
    assert_ok(&answer(request::Payload::FocusPaneAt(FocusPaneAt { place: 2 })));
    until(
        "the second pane of the window to have the keyboard",
        || showing(&named(INNER_FIRST)),
        || format!("the view still shows {:?}", shown()),
    );
}

/// Two tabs on one daemon, the second holding two panes.
///
/// Arranged so that the two schemes cannot agree: the second pane of the window sits in the
/// first tab, and the second pane of the second tab is the window's third. A test on a session
/// of one pane per tab would pass whichever scheme was in force.
fn a_session_of_two_tabs_the_second_holding_two(daemon: &Daemon) {
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "numbering", "focus": true }));
    let first = only_pane(daemon);
    daemon.call("tab.create", &json!({ "focus": false }));
    let inner_first = panes(daemon)
        .into_iter()
        .find(|pane| pane != &first)
        .expect("the new tab brings a pane of its own");
    daemon.call("pane.split", &json!({ "target_pane_id": inner_first, "direction": "right" }));
    let inner_second = panes(daemon)
        .into_iter()
        .find(|pane| pane != &first && pane != &inner_first)
        .expect("splitting makes a second pane in that tab");

    for (pane, given) in
        [(&first, VISIBLE), (&inner_first, INNER_FIRST), (&inner_second, INNER_SECOND)]
    {
        daemon.call("pane.rename", &json!({ "pane_id": pane, "label": given }));
    }
}

/// What the second tab's two panes are called, so the assertions read as the arrangement.
const INNER_FIRST: &str = "inner-first";
const INNER_SECOND: &str = "inner-second";

/// The number on every tab that carries one, in the order the roster lists them.
fn numbered_tabs() -> Vec<u32> {
    roster()
        .into_iter()
        .flat_map(|roster| roster.daemons)
        .flat_map(|daemon| daemon.tabs)
        .filter_map(|tab| (tab.number > 0).then_some(tab.number))
        .collect()
}

/// The number on every pane that carries one, in the order the roster lists them.
fn numbered_panes() -> Vec<u32> {
    roster()
        .into_iter()
        .flat_map(|roster| roster.daemons)
        .flat_map(|daemon| daemon.tabs)
        .flat_map(|tab| tab.panes)
        .filter_map(|pane| (pane.number > 0).then_some(pane.number))
        .collect()
}

/// Two tabs on one daemon, one pane each, so that the second pane is in a tab nothing shows.
///
/// Returns the pane in the tab a region will land on, then the pane behind it.
fn a_session_of_two_tabs(daemon: &Daemon) {
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "numbering", "focus": true }));
    let first = only_pane(daemon);
    daemon.call("tab.create", &json!({ "focus": false }));
    let second = panes(daemon)
        .into_iter()
        .find(|pane| pane != &first)
        .expect("the new tab brings a pane of its own");

    // Named, because this test is about which row carries which number rather than about the
    // names Muster mints. Before startup, since herdr announces a rename to nobody.
    for (pane, given) in [(&first, VISIBLE), (&second, HIDDEN)] {
        daemon.call("pane.rename", &json!({ "pane_id": pane, "label": given }));
    }
}

/// What the two panes are called, so the assertions below read as the arrangement they are about.
const VISIBLE: &str = "visible";
const HIDDEN: &str = "hidden";

/// What Muster calls the pane somebody named `given`.
fn named(given: &str) -> String {
    roster()
        .into_iter()
        .flat_map(|roster| rows(&roster))
        .find_map(|(_, name, pane)| (name == given).then_some(pane))
        .unwrap_or_else(|| panic!("the roster lists no pane called {given}"))
}

fn only_pane(daemon: &Daemon) -> String {
    let panes = panes(daemon);
    assert_eq!(panes.len(), 1, "a fresh workspace holds one pane, and held {panes:?}");
    panes.into_iter().next().expect("just counted one")
}

fn panes(daemon: &Daemon) -> Vec<String> {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    snapshot
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("panes"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no panes in {snapshot}"))
        .iter()
        .filter_map(|pane| pane.get("pane_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Every pane's place and given name, in the order the roster lists them.
fn places(roster: &RosterChanged) -> Vec<(u32, String)> {
    rows(roster).into_iter().map(|(place, given, _)| (place, given)).collect()
}

/// Every listed pane, as its place, the name somebody gave it, and the name Muster minted.
fn rows(roster: &RosterChanged) -> Vec<(u32, String, String)> {
    roster
        .daemons
        .iter()
        .flat_map(|daemon| daemon.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .map(|pane| (pane.place, pane.given_name.clone(), pane.pane_id.clone()))
        .collect()
}

/// Whether the view has this pane on screen with the keyboard on it.
fn showing(pane: &str) -> bool {
    shown().as_deref() == Some(pane)
}

fn shown() -> Option<String> {
    let view = VIEW.lock().expect("a panicking test poisoned the view");
    let view = view.as_ref()?;
    let focused = view.regions.iter().find(|region| region.region_id == view.focused_region)?;
    (!focused.pane_id.is_empty()).then(|| focused.pane_id.clone())
}

fn regions() -> usize {
    VIEW.lock()
        .expect("a panicking test poisoned the view")
        .as_ref()
        .map(|view| view.regions.len())
        .unwrap_or_default()
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);
static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);
static ROSTERS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which is
    // the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::ViewChanged(view)) => {
            *VIEW.lock().expect("a panicking test poisoned the view") = Some(view);
        }
        Some(event::Payload::RosterChanged(roster)) => {
            *ROSTER.lock().expect("a panicking test poisoned the roster") = Some(roster);
            ROSTERS.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
}

/// This test's turn, with what the last one was told forgotten.
///
/// [`muster::testing::fresh_session`] resets the core. These statics are this file's own and it
/// cannot reach them, so left alone they carry the previous test's window into this one - and a
/// test that waits for "three panes in the roster" is handed three from a window that has
/// already closed, then asserts against a core that has not started yet.
fn a_fresh_window() -> muster::testing::Turn {
    let turn = muster::testing::fresh_session();
    *VIEW.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    *ROSTER.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    ROSTERS.store(0, Ordering::Relaxed);
    turn
}

fn roster() -> Option<RosterChanged> {
    ROSTER.lock().expect("a panicking test poisoned the roster").clone()
}

/// How many times the shell has been told what exists, so a test can assert it was not.
fn rosters() -> usize {
    ROSTERS.load(Ordering::Relaxed)
}

/// Waits for the publishing to stop, and hands back the count it stopped at.
///
/// Only a test asserting that nothing was published needs this. Everything else here waits for
/// something to arrive, which is self-timing; an absence is not, and a bootstrap still landing
/// underneath one would fail it for reasons that have nothing to do with the code.
fn once_quiet() -> usize {
    let mut settled = rosters();
    until(
        "the window to stop republishing",
        || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let now = rosters();
            let quiet = now == settled;
            settled = now;
            quiet
        },
        || format!("the roster has been published {} times and is still going", rosters()),
    );
    settled
}

/// What the chords are counting, as the roster says it to the shell.
///
/// Read off the message rather than out of the session, because the question these tests are
/// about is what a window is told - a core that knows a chord is half-typed and does not say so
/// draws no badges and ends no gesture.
fn counting() -> Counting {
    roster().map_or(Counting::Panes, |roster| {
        Counting::try_from(roster.counting).expect("the core sends a counting this build knows")
    })
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

fn refusal(payload: request::Payload) -> String {
    match answer(payload).payload {
        Some(response::Payload::Failure(failure)) => failure.reason,
        other => panic!("expected the core to refuse this, and it answered {other:?}"),
    }
}
