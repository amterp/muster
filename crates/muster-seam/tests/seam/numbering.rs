//! What ⌘1 to ⌘9 mean, end to end against a real daemon.
//!
//! Every layer of this is already covered: `bindings.json` pins the nine chords, `roster.json`
//! pins the numbering they name, and `composition.json` pins what surfacing a tab does. What
//! is not covered anywhere else is the composition of those layers - which is the shape every
//! bug this project has shipped recently has had, each one green at every level and wrong when
//! the app ran.
//!
//! So this asserts the gesture: the chord the roster hands the shell is the chord the shell can
//! send back, and pressing it lands the keyboard on the pane whose row carries it - including a
//! pane in a tab nothing is showing, which reaching has to bring on screen. Every layer of a
//! two-press chord is pinned in the corpus; what only a running window shows is that the first
//! press does not quietly disarm itself on whatever it causes the shell to send back.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use muster::proto::{
    EndNumberedChord, Event, FocusPaneAt, OpenWindow, PressNumberedChord, Request, Response,
    RosterChanged, Startup, ViewChanged, WindowFocus, event, request, response,
    roster_changed::Counting,
};
use muster_daemon_proto::{Placement, Side};
use muster_harness::requests::{beside, create, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn a_numbered_chord_lands_on_the_row_carrying_it() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start_built();
    a_session_of_two_tabs(&daemon);

    // Registered before startup, because startup begins following the configured daemons and a
    // callback added afterwards misses the first bootstrap entirely.
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

    until(
        "the roster to arrive with both tabs in it",
        || roster().is_some_and(|roster| places(&roster).len() == 2),
        || format!("the roster holds {:?}", roster().map(|r| places(&r))),
    );

    // Each tab holds one pane, so reaching the tab is the whole chord: the second tab's only
    // pane carries ⌘2 and nothing after it. Read off the roster rather than assumed, because
    // the whole point is that the shell sends back the press it was given.
    assert_eq!(chords(), vec![vec![1], vec![2]], "each pane should be one press onto its tab");
    let hidden = named(HIDDEN);

    // The tab holding it is not on screen: one region, showing the first tab. That is the case
    // the chords exist for.
    assert!(
        !showing(&hidden),
        "this test is pointless unless the second tab starts hidden, and the view already shows it"
    );

    assert_ok(&answer(request::Payload::PressNumberedChord(PressNumberedChord { press: 2 })));

    until(
        "the hidden pane to be on screen with the keyboard",
        || showing(&hidden),
        || format!("the view still shows {:?}", shown()),
    );

    // Surfaced rather than opened beside: a second region onto the same daemon would be two
    // copies of one window, and the region count is what tells those apart.
    assert_eq!(regions(), 1, "reaching a hidden pane opened a region instead of retargeting one");

    // And the refusal, in the same breath, because a press past the end is what ⌘9 means in a
    // window of two tabs and it has to do nothing rather than land somewhere.
    let reason = refusal(request::Payload::PressNumberedChord(PressNumberedChord { press: 9 }));
    assert!(
        reason.contains("2 tabs") && reason.contains("no tab 9"),
        "a press past the end should say how many tabs there are, and said: {reason}"
    );
    assert!(
        showing(&hidden),
        "a refused chord moved the keyboard, so it did something rather than nothing"
    );
}

/// `muster focus --place` goes to the pane `muster window` prints at that place, whatever the
/// numbered chords are naming.
///
/// A script reads a place off `muster window` and hands it back, so the number has to mean
/// what it was read as. The chord ⌘3 names the third tab, and a place
/// resolved through the chords would send `--place 3` there - or nowhere, in a window of two.
#[test]
fn a_place_is_the_pane_at_that_place_whatever_the_chords_name() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start_built();
    a_session_of_two_tabs_the_second_holding_two(&daemon);

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the roster to arrive with all three panes in it",
        || roster().is_some_and(|roster| rows(&roster).len() == 3),
        || format!("the roster holds {:?}", roster().map(|held| places(&held))),
    );
    let third = rows(&roster().expect("just waited for it"))
        .into_iter()
        .find_map(|(place, _, pane)| (place == 3).then_some(pane))
        .expect("the roster numbers three panes");
    assert_eq!(third, named(INNER_SECOND), "the arrangement this test needs came apart");

    assert_ok(&answer(request::Payload::FocusPaneAt(FocusPaneAt { place: 3 })));
    until(
        "the pane at place 3 to have the keyboard",
        || showing(&third),
        || format!("the view still shows {:?}", shown()),
    );
    assert_eq!(armed_tabs(), Vec::<u32>::new(), "going to a place armed a chord");
}

/// A window of one tab, where the first press would name that tab.
///
/// The collapse (kan a_2Hx68fXqr): ⌘1 naming the only tab there is spends a press on nothing,
/// so a window holding one tab numbers panes instead. What only a running window can show is
/// the second half of it - that the shell is not left holding a half-typed chord. `counting`
/// is what the shell reads to decide whether to draw a number over every pane and whether
/// releasing ⌘ ends a gesture, so a collapse spelled as "the panes in that tab" would leave
/// those numbers drawn over a window nobody had pressed anything in.
#[test]
fn one_tab_numbers_its_panes_and_arms_nothing() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start_built();
    a_session_of_one_tab_holding_two(&daemon);

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

    until(
        "the roster to arrive with both panes in it",
        || roster().is_some_and(|roster| rows(&roster).len() == 2),
        || format!("the roster holds {:?}", roster().map(|held| places(&held))),
    );

    // One press each, and it names the pane rather than the tab: a chord of two presses here
    // would say the window had not collapsed.
    assert_eq!(chords(), vec![vec![1], vec![2]], "the chords should be naming panes, not the tab");
    assert_eq!(tab_presses(), Vec::<u32>::new(), "the only tab there is carried a press");
    assert_eq!(
        counting(),
        Counting::Panes,
        "the window says a chord is half-typed with nothing pressed, so every pane is wearing \
         a number and the next ⌘ release ends a gesture nobody started"
    );

    // One press, and it is the whole gesture. Without the collapse this would have named the
    // tab and left the window waiting for a second press.
    assert_ok(&answer(request::Payload::PressNumberedChord(PressNumberedChord { press: 2 })));
    until(
        "the second pane of the only tab to have the keyboard",
        || showing(&named(INNER_SECOND)),
        || format!("the view still shows {:?}", shown()),
    );
    assert_eq!(counting(), Counting::Panes, "landing on a pane left a chord armed behind it");
}

#[test]
fn a_tab_is_named_first_and_a_pane_inside_it_second() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start_built();
    a_session_of_two_tabs_the_second_holding_two(&daemon);

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

    until(
        "the roster to arrive with all three panes in it",
        || roster().is_some_and(|roster| rows(&roster).len() == 3),
        || format!("the roster holds {:?}", roster().map(|held| places(&held))),
    );

    // Every row's whole address, before anything is pressed. The first tab holds one pane and
    // so does not arm, which is why its pane's chord is the one press onto the tab: ⌘1 ⌘1 would
    // be two tab jumps rather than that pane.
    let addresses = vec![vec![1], vec![2, 1], vec![2, 2]];
    assert_eq!(tab_presses(), vec![1, 2], "the tabs should be reachable before any press");
    assert_eq!(chords(), addresses, "a pane in a tab nothing shows should still have an address");
    assert_eq!(armed_tabs(), Vec::<u32>::new(), "nothing was pressed and a tab is already armed");

    let inner_second = named(INNER_SECOND);
    assert_ok(&answer(request::Payload::PressNumberedChord(PressNumberedChord { press: 2 })));

    // Acted on immediately rather than waiting for a second press: the tab is on screen and the
    // keyboard is on its first pane, which is where a click on its caption would have put it.
    until(
        "the second tab to be on screen with the keyboard on its first pane",
        || showing(&named(INNER_FIRST)),
        || format!("the view still shows {:?}", shown()),
    );

    // And nothing has moved. The second press was legible before the first one was made, so
    // what a press changes is which of those addresses is live, not where they are drawn - the
    // whole of kan a_2LSUoy7dd in two assertions.
    until(
        "the tab the press named to be armed",
        || armed_tabs() == vec![2],
        || format!("the armed tabs carry {:?}", armed_tabs()),
    );
    assert_eq!(tab_presses(), vec![1, 2], "a tab lost its press to a chord being half-typed");
    assert_eq!(chords(), addresses, "the addresses moved under somebody reading them");

    assert_ok(&answer(request::Payload::PressNumberedChord(PressNumberedChord { press: 2 })));
    until(
        "the second pane of the second tab to have the keyboard",
        || showing(&inner_second),
        || format!("the view still shows {:?}", shown()),
    );

    // Two presses counted down the whole window would have landed on its second pane twice
    // over, which is a different pane, so this cannot pass by counting the wrong way.
    assert_ne!(inner_second, named(INNER_FIRST), "the arrangement this test needs came apart");
}

#[test]
fn anything_between_the_two_presses_takes_the_first_one_back() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start_built();
    a_session_of_two_tabs_the_second_holding_two(&daemon);

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the roster to arrive with all three panes in it",
        || roster().is_some_and(|roster| rows(&roster).len() == 3),
        || format!("the roster holds {:?}", roster().map(|held| places(&held))),
    );

    assert_ok(&answer(request::Payload::PressNumberedChord(PressNumberedChord { press: 2 })));
    until(
        "the second tab to be named",
        || armed_tabs() == vec![2],
        || format!("the armed tabs carry {:?}", armed_tabs()),
    );

    // The window losing focus is one of the ordinary things that happen between two keystrokes,
    // and it stands here for all of them: the rule is that anything which is not a read takes
    // the arm back. Asserted through what the window publishes rather than through where the
    // next press lands, because that is what a person is reading while deciding to press.
    assert_ok(&answer(request::Payload::WindowFocus(WindowFocus { focused: false })));
    until(
        "the arm to be taken back",
        || armed_tabs().is_empty(),
        || format!("the armed tabs carry {:?}", armed_tabs()),
    );

    // So the press that follows is a first press again, and reaches a tab rather than a pane.
    assert_ok(&answer(request::Payload::PressNumberedChord(PressNumberedChord { press: 1 })));
    until(
        "the first tab's only pane to have the keyboard",
        || showing(&named(VISIBLE)),
        || format!("the view still shows {:?}", shown()),
    );
}

#[test]
fn letting_go_of_the_modifier_takes_the_first_press_back() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start_built();
    a_session_of_two_tabs_the_second_holding_two(&daemon);

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the roster to arrive with all three panes in it",
        || roster().is_some_and(|roster| rows(&roster).len() == 3),
        || format!("the roster holds {:?}", roster().map(|held| places(&held))),
    );
    assert_eq!(counting(), Counting::Tabs, "the chords should be naming tabs before any press");

    assert_ok(&answer(request::Payload::PressNumberedChord(PressNumberedChord { press: 2 })));
    until(
        "the second tab to be named",
        || counting() == Counting::PanesInTab,
        || format!("the roster says the chords are counting {:?}", counting()),
    );
    // What releasing ⌘ means. Distinct from every other way a chord ends, because it is the one
    // that happens when somebody decides mid-gesture that the tab was all they wanted - and
    // until this existed, walking away from the keyboard there left the window waiting.
    assert_ok(&answer(request::Payload::EndNumberedChord(EndNumberedChord {})));
    until(
        "the arm to be taken back",
        || armed_tabs().is_empty(),
        || format!("the armed tabs carry {:?}", armed_tabs()),
    );
    assert_eq!(counting(), Counting::Tabs, "the roster still says a chord is half-typed");

    // So the press after it is a first press again, and reaches a tab rather than a pane. This
    // is the whole complaint the change answers: ⌘2, let go, ⌘1 should be two tab jumps.
    assert_ok(&answer(request::Payload::PressNumberedChord(PressNumberedChord { press: 1 })));
    until(
        "the first tab's only pane to have the keyboard",
        || showing(&named(VISIBLE)),
        || format!("the view still shows {:?}", shown()),
    );
}

#[test]
fn ending_a_chord_nobody_started_says_nothing() {
    let _turn = a_fresh_window();
    let daemon = Daemon::start_built();
    a_session_of_two_tabs_the_second_holding_two(&daemon);

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
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

/// Two tabs on one daemon, the second holding two panes.
///
/// Arranged so that the two schemes cannot agree: the second pane of the window sits in the
/// first tab, and the second pane of the second tab is the window's third. A test on a session
/// of one pane per tab would pass whichever scheme was in force.
fn a_session_of_two_tabs_the_second_holding_two(daemon: &Daemon) {
    labelled(daemon, "p1", in_new_tab("t1"), VISIBLE);
    labelled(daemon, "p2", in_new_tab("t2"), INNER_FIRST);
    labelled(daemon, "p3", beside("p2", Side::Right), INNER_SECOND);
}

/// One tab holding two panes, which is the window a person opens Muster to.
///
/// Named with the same two words the second tab's panes carry above, because they are the same
/// two rows to every assertion here: the second pane of the tab the keyboard is in.
fn a_session_of_one_tab_holding_two(daemon: &Daemon) {
    labelled(daemon, "p1", in_new_tab("t1"), INNER_FIRST);
    labelled(daemon, "p2", beside("p1", Side::Right), INNER_SECOND);
}

/// What the second tab's two panes are called, so the assertions read as the arrangement.
const INNER_FIRST: &str = "inner-first";
const INNER_SECOND: &str = "inner-second";

/// The press on every tab that carries one, in the order the roster lists them.
fn tab_presses() -> Vec<u32> {
    roster()
        .into_iter()
        .flat_map(|roster| roster.tabs)
        .filter_map(|tab| (tab.tab_press > 0).then_some(tab.tab_press))
        .collect()
}

/// The presses that reach every pane, in the order the roster lists them.
///
/// One entry per pane whether or not anything reaches it, unlike the tabs above: what these
/// tests are about is that the entries do not change as a chord is typed, and a list that
/// dropped the empty ones would say that by having a different length instead.
fn chords() -> Vec<Vec<u32>> {
    roster()
        .into_iter()
        .flat_map(|roster| roster.tabs)
        .flat_map(|tab| tab.panes)
        .map(|pane| [pane.tab_press, pane.pane_press].into_iter().filter(|&at| at > 0).collect())
        .collect()
}

/// The tabs whose panes the next press would name, said as the press that reaches each.
///
/// What a window says in place of moving its numbers, and so the observable these tests use for
/// a first press having landed and not yet been spent.
fn armed_tabs() -> Vec<u32> {
    roster()
        .into_iter()
        .flat_map(|roster| roster.tabs)
        .filter(|tab| tab.armed)
        .map(|tab| tab.tab_press)
        .collect()
}

/// Two tabs on one daemon, one pane each, so that the second pane is in a tab nothing shows.
fn a_session_of_two_tabs(daemon: &Daemon) {
    // Named, because this test is about which row carries which number rather than about the
    // names Muster mints.
    labelled(daemon, "p1", in_new_tab("t1"), VISIBLE);
    labelled(daemon, "p2", in_new_tab("t2"), HIDDEN);
}

/// Makes a pane on the daemon, before any window opens, under a name somebody gave it.
fn labelled(daemon: &Daemon, pane: &str, placement: Placement, given: &str) {
    let mut request = create(pane, placement);
    request.label = Some(given.to_string());
    make(&mut daemon.connect(), request);
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

/// Every pane's place and given name, in the order the roster lists them.
fn places(roster: &RosterChanged) -> Vec<(u32, String)> {
    rows(roster).into_iter().map(|(place, given, _)| (place, given)).collect()
}

/// Every listed pane, as its place, the name somebody gave it, and the name Muster minted.
fn rows(roster: &RosterChanged) -> Vec<(u32, String, String)> {
    roster
        .tabs
        .iter()
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
    let bytes = Request::new(payload).encode_to_vec();
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
        Some(
            response::Payload::Ok(_) | response::Payload::Made(_) | response::Payload::Opened(_),
        ) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}

fn refusal(payload: request::Payload) -> String {
    match answer(payload).payload {
        Some(response::Payload::Failure(failure)) => failure.reason,
        other => panic!("expected the core to refuse this, and it answered {other:?}"),
    }
}
