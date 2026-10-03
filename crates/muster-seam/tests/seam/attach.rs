//! What attaching settles, against a real daemon.
//!
//! Attaching is where composition meets a backend: the core is handed a pane id and has to
//! turn it into a region showing that pane's tab, with the keyboard pointed at it. Only the
//! daemon knows which tab that is, so this is the one part of composition a recorded case
//! cannot judge - `composition.json` covers everything downstream of the answer, and this
//! covers getting one.
//!
//! The refusals matter as much as the success. A window attached to a pane no daemon holds
//! renders nothing and ignores the keyboard, which is indistinguishable from every other
//! way this can go wrong and is the symptom that has cost this project the most time.
//!
//! One test per behaviour, each with a daemon and a session of its own. They were nine
//! scenarios chained through one test until `muster::testing` existed, because the seam holds
//! its session in a process global - and a chained scenario stops at the first failure, so
//! everything after it went unrun and one red run could not say whether that broke too. The
//! turn each takes is what serialises them; the fixture below is what they share.

use std::collections::BTreeSet;
use std::sync::Mutex;

use muster::proto::{
    AttachPane, ClosePane, CreateTab, Event, FocusPane, OpenWindow, Paste, ReadWindow, Request,
    Response, RosterChanged, SplitPane, Startup, ViewChanged, ViewNode, WindowFocus, ZoomPane,
    event, request, response, view_node,
};
use muster_daemon_proto::{AgentState, Side};
use muster_harness::requests::{
    beside, close_request, create, in_new_tab, make, read_text, snapshot,
};
use muster_harness::{Daemon, until};
use prost::Message;

/// The panes the session opens with: two side by side in one tab, which is what every test
/// here attaches to.
const FIRST: &str = "p1";
const SECOND: &str = "p2";

/// A window on a session that was already running, which is where every test here starts.
///
/// The daemon comes back with it because dropping one kills it, so a test that let go of it
/// would be a test whose panes stop existing halfway through. The daemon knows each pane by the
/// name the test gave it, which is also what Muster calls it, so one spelling serves the
/// requests and the oracles alike.
struct Open {
    daemon: Daemon,
}

/// The same window, over a session whose first pane has an agent that was already working
/// before Muster was started.
fn a_window_onto_work_already_running() -> Open {
    a_window_onto(|daemon| {
        daemon.run_agent(FIRST);
        daemon.set_agent_state(FIRST, AgentState::Working);
    })
}

/// A window onto two plain shells, for a test that needs no agent before it starts.
fn a_window_onto_two_shells() -> Open {
    a_window_onto(|_| {})
}

/// Builds the session, lets `before` add to it while Muster has heard of none of it, and starts
/// a window on it.
fn a_window_onto(before: impl FnOnce(&Daemon)) -> Open {
    // Detecting, so a test can run an agent the daemon recognises. Nothing detects anything in a
    // pane that runs no agent, so the others pay nothing for it.
    let daemon = Daemon::start_detecting();
    a_session_with_work_already_in_it(&daemon);
    before(&daemon);

    // A config file naming this daemon's socket, which is how a person points Muster at a
    // daemon it did not start.
    let config = daemon.muster_config();
    // Before startup, because that is the order the shell uses (`Sources/MusterMac/Core.swift`)
    // and the order is load-bearing: startup begins following the configured daemons, so a
    // callback registered after it misses the whole first bootstrap - every pane that already
    // existed, and whatever their agents were already doing.
    watch();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    // Attaching finds a pane in what the window has heard from its daemons, so every test starts
    // once the window has heard of both. Asked rather than waited for in a roster, because a
    // window sends none until it has opened, and attaching is what opens it here.
    for pane in [FIRST, SECOND] {
        until(
            &format!("the core to know of {pane}"),
            || known_panes().iter().any(|known| known == pane),
            || format!("the core knows of {:?}", known_panes()),
        );
    }

    Open { daemon }
}

/// Starts collecting what the core pushes, from nothing.
///
/// The collectors below are statics, so they outlive a test the way the session used to - and a
/// roster left by the last test is one that answers instantly with a pane this daemon has
/// never held.
fn watch() {
    *VIEW.lock().expect("a panicking reader poisoned the view") = None;
    STATES.lock().expect("a panicking reader poisoned the states").clear();
    *ROSTER.lock().expect("a panicking reader poisoned the roster") = None;
    muster::ffi::muster_set_event_callback(Some(note_view));
}

/// Attaching refuses what it cannot show and says why, places a pane that exists with the
/// keyboard on it, gives each pane a socket of its own, and brings along the agent states that
/// predate the window.
#[test]
fn attaching_places_a_pane_where_the_keyboard_can_find_it() {
    let _turn = muster::testing::fresh_session();
    // Before the window has heard of any pane, so this is the state a window is in on the way
    // up rather than one it fell back to. Asked before startup, because a window that has heard
    // its daemon describe a tab shows it at once, and the moment in between is a race.
    let reason =
        refusal(request::Payload::Paste(Paste { text: "hello".to_string(), ..Paste::default() }));
    assert!(
        reason.contains("no pane has this window's keyboard"),
        "input with nothing attached should say so, and said: {reason}"
    );

    let Open { daemon } = a_window_onto_work_already_running();

    let reason =
        refusal(request::Payload::AttachPane(AttachPane { pane_id: "p9nobody00".to_string() }));
    assert!(
        reason.contains("p9nobody00") && reason.contains("run `muster`"),
        "a pane no daemon holds should be refused by name, and was refused with: {reason}"
    );
    // What a person is told to do next is Muster's own, never the backend's. Asserted rather
    // than left to review because this message is read at exactly the moment somebody is
    // confused, and naming the daemon's own machinery there teaches them a vocabulary Muster
    // exists to spare them (README desiderata, swappable organs).
    assert!(
        !reason.contains("muster-daemon"),
        "a refusal a user reads should not hand them the backend's own terms: {reason}"
    );

    let one = attach(FIRST);
    assert!(
        std::path::Path::new(&one.link_socket_path).exists(),
        "the bridge's socket is bound before attach returns, and {} is not there",
        one.link_socket_path
    );
    // A second pane in the same tab. Two things are being asserted at once because they are
    // the same mistake: a socket per process rather than per pane would hand back the path
    // it already gave out, and one bridge would be reporting for both panes.
    let two = attach(SECOND);
    assert_ne!(
        one.link_socket_path, two.link_socket_path,
        "each pane reports to the core on its own socket, and both panes were given one path"
    );

    // The agent that was working before any of this began. Bootstrap says only that the
    // pane appeared, so a core that told the shell about transitions alone would leave this
    // window painting a busy agent as unknown grey until it happened to move again - and a
    // window opened onto running work is exactly when the states have to be right.
    until(
        "the working agent that predates this window to reach the shell",
        || latest_state(FIRST).as_deref() == Some("working"),
        || format!("the core last said {:?} about {FIRST}", latest_state(FIRST)),
    );

    // The keyboard follows the pane just attached, which is the whole of composition doing
    // its job: a region for the tab, a view-local cursor in it, and a lookup that found the
    // attachment behind it.
    //
    // Asserted on the panes rather than on the answer, because the answer is `ok` either
    // way - the seam reports that it found somewhere to send, not where. Text sent to one
    // pane and not the other is visible on exactly one screen, and the wrong-pane bug is the
    // one that looks like nothing at all from here. Input goes to the daemon on the window's
    // own connection rather than through a bridge, so the daemon's reading of the pane is the
    // whole oracle and no bridge is needed.
    //
    // The second pane's shell first. A pane's program is spawned when the pane is created, so
    // text pasted before its shell has drawn a prompt races the program's own first output -
    // which is how this passed alone and failed under a loaded suite.
    until(
        "the second pane's shell to come up",
        || !screen(&daemon, SECOND).trim().is_empty(),
        || format!("{SECOND}: {:?}", screen(&daemon, SECOND)),
    );

    // Short enough to fit a split pane's width beside a shell prompt.
    let typed = "mstr-here";
    assert_ok(&answer(request::Payload::Paste(Paste {
        text: typed.to_string(),
        ..Paste::default()
    })));
    until(
        "the text to appear in the pane that has the keyboard",
        || screen(&daemon, SECOND).contains(typed),
        || {
            format!(
                "{FIRST}: {:?}\n{SECOND}: {:?}",
                screen(&daemon, FIRST),
                screen(&daemon, SECOND)
            )
        },
    );
    assert!(
        !screen(&daemon, FIRST).contains(typed),
        "the keyboard should follow the pane just attached, and the text landed in {FIRST} \
         as well as, or instead of, {SECOND}"
    );
}

/// Closing the last pane, and getting one back without asking.
///
/// A window with no panes was a window nobody could refill. Every request is about a pane -
/// a split splits one, a close closes one, and a new tab used to need one to say where to put
/// it - so the answer to all of them was the same refusal, and the way out of an empty window
/// was to quit and relaunch.
///
/// Nothing is asked for here, and that is the assertion. A window that is showing nothing asks
/// its first machine for a tab itself (kan a_2HpkpfIfq), so the empty state is one Muster
/// passes through rather than one it can be left in. ⌘T is no longer the way out and no
/// longer waited for; a version of this that sent one would pass whether or not the rule under
/// test did anything.
///
/// Driven through the daemon rather than through the core's own close, because what is under
/// test is what a window does once it is empty, and this reaches that state the way the
/// commonest one does: the daemon lost the panes and said so.
#[test]
fn an_emptied_window_refills_itself() {
    let _turn = muster::testing::fresh_session();
    let Open { daemon } = a_window_onto_two_shells();
    let daemon = &daemon;

    // The rule that refills an emptied window waits for the window to say what it is showing,
    // and this is the only test here that needs it to have said so: the others assert about a
    // window on its way up, which is a real state and the one the guard exists for. Sent here
    // rather than in the shared helper for that reason - opening it there takes the pre-attach
    // state away from the test that is about it.
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

    let before = panes(daemon);
    let mut control = daemon.connect();
    for pane in &before {
        control.ask(close_request(pane));
    }
    // Waited for on the daemon rather than on the view, and for a pane that is *not* one of the
    // originals: every pane the window opened with is on screen when this starts, so a wait on
    // the view showing something would be satisfied before a single close had landed.
    until(
        "the emptied machine to be given a pane back",
        || panes(daemon).iter().all(|pane| !before.contains(pane)) && !panes(daemon).is_empty(),
        || format!("the daemon holds {:?}, and held {before:?}", panes(daemon)),
    );
    assert_eq!(
        panes(daemon).len(),
        1,
        "an emptied machine was given more than one tab, so the rule that asks is asking again \
         while its own answer is still in flight"
    );

    // And the window is showing it with the keyboard on it, which is the half a person sees.
    until(
        "the window to show the pane it was given, with the keyboard on it",
        || {
            latest_view().is_some_and(|view| {
                view.regions.first().is_some_and(|region| !region.pane_id.is_empty())
            })
        },
        || format!("the last view the core published: {:?}", latest_view()),
    );
}

/// Making a tab, and the window moving onto it.
///
/// The window names the tab itself and asks the daemon for it, and the daemon's events about it
/// reach the window before its answer does - so by the time the request returns, the tab is
/// already the one on screen, with the keyboard on its pane. Asserted straight after the
/// request rather than waited for, because a window that showed the tab only on some later
/// event is one where a keystroke sent right after ⌘T lands in the tab that was left.
#[test]
fn a_new_tab_is_made_and_then_shown() {
    let _turn = muster::testing::fresh_session();
    let Open { daemon } = a_window_onto_two_shells();
    let daemon = &daemon;
    // A window showing something, which every one of these starts from.
    attach(SECOND);

    let before =
        latest_view().expect("the window is showing something by now").regions[0].tab_id.clone();
    let tabs_before = tab_count(daemon);

    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        daemon_id: String::new(),
        pane_id: String::new(),
        cwd: String::new(),
        run: String::new(),
        name: String::new(),
        take_focus: true,
    })));

    let view = latest_view().expect("the window was showing something before it asked");
    let region = view.regions.first().expect("the window shows a region");
    assert!(
        region.tab_id != before && !region.pane_id.is_empty(),
        "the window did not move onto the tab it asked for by the time the request returned; \
         it still shows {before}. The view: {view:?}"
    );
    // One region, not two: a new tab is somewhere this window goes, not a second copy of the
    // window beside the first.
    assert_eq!(
        view.regions.len(),
        1,
        "a new tab opened a second region instead of moving the one that asked for it"
    );
    assert_eq!(
        tab_count(daemon),
        tabs_before + 1,
        "the daemon holds a different number of tabs than one more than before, so the tab \
         was made somewhere else or made twice"
    );
}

/// How many tabs this daemon holds, by its own account.
fn tab_count(daemon: &Daemon) -> usize {
    snapshot(&mut daemon.connect()).tabs.len()
}

/// The session this window opens onto, built before Muster has heard of any of it.
///
/// The ordinary case rather than a contrivance: the daemon outlives the app, so most windows
/// open onto panes whose agents have been running for a while. Everything here happens before
/// the core starts watching, so nothing below is explained by a transition it saw.
fn a_session_with_work_already_in_it(daemon: &Daemon) {
    let mut control = daemon.connect();
    make(&mut control, create(FIRST, in_new_tab("t1")));
    make(&mut control, create(SECOND, beside(FIRST, Side::Right)));
}

/// Going to a pane in a tab this window is not showing.
///
/// The half of attention routing that is not a colour. Glanceable states are the floor: an
/// agent that finished or is waiting for somebody is most often on a pane no region is
/// showing, and being told about it only helps if going there works. Before this, focusing
/// such a pane was refused by name - which is a list of things you cannot reach.
#[test]
fn a_pane_no_region_shows_can_still_be_reached() {
    let _turn = muster::testing::fresh_session();
    let Open { daemon } = a_window_onto_two_shells();
    let daemon = &daemon;
    // A window showing something, which every one of these starts from.
    attach(SECOND);

    let before = latest_view().expect("the window is showing something by now");
    make(&mut daemon.connect(), create("p3", in_new_tab("t2")));

    // Listed, and listed as hidden - which is the row the sidebar would draw and the state
    // this whole check is about. Waited for on the list rather than on the view, because the
    // view is the one place this pane will never appear until something surfaces it.
    until(
        "the new tab's pane to be listed as something nothing is showing",
        || hidden_pane().is_some(),
        || format!("the list holds {:?}", listed_panes()),
    );
    let elsewhere = hidden_pane().expect("the wait above returned because there was one");

    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        daemon_id: String::new(),
        pane_id: elsewhere.clone(),
    })));

    // One region still, retargeted rather than added: switching tabs on the daemon you are
    // already looking at should not split the window in two.
    until(
        "the region to be showing the pane that was asked for",
        || {
            latest_view()
                .is_some_and(|view| view.regions.len() == 1 && view.regions[0].pane_id == elsewhere)
        },
        || format!("the last view: {:?}", latest_view()),
    );
    // And the list agrees with the window it sits beside, which is the join the sidebar
    // draws: the row that said hidden a moment ago now says it is showing.
    until(
        "the list to agree that the pane is on screen",
        || listed(&elsewhere) == Some(true),
        || format!("the list says {:?} about {elsewhere}", listed(&elsewhere)),
    );

    // Back where it started, so that what follows is about the tab it was written against.
    // Going back is the same mechanism in reverse and is worth one assertion of its own -
    // a surface that could only move away from where you were would be a trap.
    let home = before.regions[0].pane_id.clone();
    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        daemon_id: String::new(),
        pane_id: home.clone(),
    })));
    until(
        "the window to come back to the tab it started on",
        || latest_view().is_some_and(|view| view.regions[0].pane_id == home),
        || format!("the last view: {:?}", latest_view()),
    );
}

/// An agent that finishes while nobody is looking, and what happens when somebody looks.
///
/// The half of agent state no daemon can answer alone. The daemon keeps the finish, but it
/// cannot see whether anybody was looking - so `done` holds until a window that has the
/// keyboard shows the pane, and that window's own focus is what decides it.
///
/// The settling assertion is the one that cannot pass by accident. A window that has not been
/// told it is focused has shown nobody anything, so nothing but the focus it is told about
/// can settle this.
#[test]
fn an_agent_finishing_unseen_waits_to_be_noticed() {
    let _turn = muster::testing::fresh_session();
    let Open { daemon } = a_window_onto_two_shells();
    let daemon = &daemon;
    attach(SECOND);
    daemon.run_agent(SECOND);

    daemon.set_agent_state(SECOND, AgentState::Working);
    until(
        "the agent to reach the shell as working",
        || latest_state(SECOND).as_deref() == Some("working"),
        || format!("the core last said {:?} about {SECOND}", latest_state(SECOND)),
    );

    // Nothing has told the core this window is focused, which is where it starts and where a
    // window that has not yet been looked at genuinely is.
    daemon.set_agent_state(SECOND, AgentState::Idle);
    until(
        "the finished agent to be waiting for somebody",
        || latest_state(SECOND).as_deref() == Some("done"),
        || format!("the core last said {:?} about {SECOND}", latest_state(SECOND)),
    );

    assert_ok(&answer(request::Payload::WindowFocus(WindowFocus { focused: true })));
    until(
        "looking at the pane to settle what it was waiting for",
        || latest_state(SECOND).as_deref() == Some("idle"),
        || format!("the core last said {:?} about {SECOND}", latest_state(SECOND)),
    );
}

/// Moving the keyboard inside a zoomed tab.
///
/// The pane on screen has to be the pane being typed into, and it once was not: a backend
/// spelled zoom as a flag beside its own focused pane, and Muster was reading that cursor - so
/// ⌘2 in a zoomed tab left the previous pane filling the region while the keyboard fed one
/// nobody could see. The daemon now names the zoomed pane itself, and which pane fills a
/// region is still this window's answer, because the keyboard is.
///
/// Against a real daemon rather than only in `composition.json`, because the case turns on what
/// the daemon does and does not announce, and a recorded world cannot be wrong about that in
/// the way a real one once was.
#[test]
fn zoom_follows_the_keyboard() {
    let _turn = muster::testing::fresh_session();
    let Open { daemon: _daemon } = a_window_onto_two_shells();
    attach(SECOND);

    until(
        "the tab's tree to settle at both panes",
        || settled(2).is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    assert_ok(&answer(request::Payload::ZoomPane(ZoomPane::default())));
    until(
        "the region to be filled by the pane the keyboard is on",
        || zoomed_pane().as_deref() == Some(SECOND),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // The whole of it. Nothing about the zoom was touched - the tab is still zoomed onto the
    // other pane as far as the daemon knows - so a window reading the daemon's answer stays on
    // the pane it was already showing.
    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        daemon_id: String::new(),
        pane_id: FIRST.to_string(),
    })));
    until(
        "the zoom to follow the keyboard onto the other pane",
        || zoomed_pane().as_deref() == Some(FIRST),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // And the socket follows too. A link is bound for the panes a region draws, so which pane
    // a zoom shows decides which pane has one - and a pane published without a socket is one
    // the shell must not start a bridge for, so the keyboard would land on a pane that never
    // paints.
    let filling = zoomed_leaf().expect("just waited for the region to be filled by one pane");
    assert!(
        !filling.link_socket_path.is_empty(),
        "the keyboard moved onto {} and its link socket did not: {filling:?}",
        filling.pane_id
    );
}

/// The pane filling the region, when one is - and nothing when the region is showing its tree.
fn zoomed_pane() -> Option<String> {
    zoomed_leaf().map(|pane| pane.pane_id)
}

/// The same, with everything else a shell is told about that pane.
fn zoomed_leaf() -> Option<muster::proto::ViewPane> {
    let region = latest_view()?.regions.into_iter().next()?;
    if !region.zoomed {
        return None;
    }
    match region.root?.node? {
        view_node::Node::Pane(pane) => Some(pane),
        // A zoomed region publishes the one pane, so a split here is the resolution not having
        // happened at all - which is the bug this is about, and it is not an answer.
        view_node::Node::Split(_) => None,
    }
}

/// The view the core publishes, and the two directions it moves in: following a split another
/// client made, and asking for splits and a close of its own, with the keyboard landing where a
/// chord says it should.
#[test]
fn the_window_follows_and_drives_the_tree() {
    let _turn = muster::testing::fresh_session();
    let Open { daemon } = a_window_onto_two_shells();
    let daemon = &daemon;
    attach(SECOND);

    // Both panes in one region, because a region shows a tab and both panes are in it. A
    // second region here would mean attaching a pane opened a second copy of its tab.
    until(
        "the tab's tree to settle at two leaves",
        || settled(2).is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let view = latest_view().expect("attaching publishes what the window is showing");
    assert_eq!(view.regions.len(), 1, "one tab, one region: {view:?}");
    assert_eq!(view.focused_region, view.regions[0].region_id);
    assert_eq!(view.regions[0].pane_id, SECOND, "the keyboard is on the pane just attached");

    // A split made from another client. Nothing here asked Muster for it, which is the
    // point twice over: the view follows the daemon rather than Muster's own record of what
    // it did, and the pane it grew gets a socket although nobody attached to it. Without
    // that, a shell rendering a surface per leaf would build one that never paints.
    make(&mut daemon.connect(), create("p3", beside(SECOND, Side::Right)));
    until(
        "a third leaf, with a socket of its own, to reach the window",
        || settled(3).is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // And now the other direction: Muster asks. A split named no pane, which means the one
    // the keyboard is on - what a keybinding means. Nothing about the window is applied
    // here; the fourth leaf arrives because the daemon said so, and it has said so by the
    // time the request returns.
    assert_ok(&answer(request::Payload::SplitPane(SplitPane {
        side: "down".to_string(),
        // What a chord sends. The field defaults to false because a script means false, so a
        // test about where the keyboard lands has to say which caller it is standing in for.
        take_focus: true,
        ..SplitPane::default()
    })));
    // Where it landed, not just that something did. Every other split in this tab is a
    // column, so the pane the keyboard was on ending up under a row is the one arrangement
    // that could only have come from this request, aimed at this pane. A fourth leaf alone
    // would be satisfied by a split spelled wrong, or aimed at somebody else's pane.
    assert!(
        settled(4).is_some() && parent_axis(SECOND).as_deref() == Some("rows"),
        "{SECOND} sits under {:?} once the split returned, where it should be under rows; the \
         last view: {:?}",
        parent_axis(SECOND),
        latest_view()
    );

    // The keyboard follows what you made. Leftward, so the pane made comes before the one
    // split in reading order - a keyboard left on the pane that was split, or put on whichever
    // leaf is last, cannot pass for this. What a miss looks like in the window is a new pane
    // appearing unfocused while the keyboard sits in the pane you split.
    let before: BTreeSet<String> =
        settled(4).expect("just asserted it").into_iter().map(|(id, _)| id).collect();
    assert_ok(&answer(request::Payload::SplitPane(SplitPane {
        side: "left".to_string(),
        // What a chord sends, as above.
        take_focus: true,
        ..SplitPane::default()
    })));
    let landed = || -> Option<bool> {
        let panes = settled(5)?;
        let made = panes.iter().map(|(id, _)| id).find(|id| !before.contains(*id))?;
        Some(&latest_view()?.regions.into_iter().next()?.pane_id == made)
    };
    assert_eq!(
        landed(),
        Some(true),
        "the keyboard did not land on the pane the split made by the time it returned; the \
         panes before the split were {before:?}; the last view: {:?}",
        latest_view()
    );

    // Closing names a pane, the way a CLI would.
    let doomed = settled(5).expect("just asserted it")[0].0.clone();
    assert_ok(&answer(request::Payload::ClosePane(ClosePane {
        daemon_id: String::new(),
        pane_id: doomed.clone(),
    })));
    // Waited for, unlike the splits. The daemon's events reach the mirror before its answer,
    // but a close moves no keyboard, so nothing republishes the view inside the request: the
    // view that drops the pane comes from the window following its daemon.
    until(
        "the closed pane to leave the window",
        || settled(4).is_some_and(|panes| panes.iter().all(|(id, _)| id != &doomed)),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // A refusal is a refusal, not a silent no-op. Nothing this window shows holds that pane,
    // so there is no daemon to ask - which is the state a stale intent arrives in.
    let reason = refusal(request::Payload::ClosePane(ClosePane {
        daemon_id: String::new(),
        pane_id: doomed.clone(),
    }));
    assert!(
        reason.contains("is not showing that pane or tab"),
        "a request for a pane that is gone should say so, and said: {reason}"
    );
}

/// The published view's one region, once its tree has exactly `leaves` panes and each of
/// them names a socket of its own.
///
/// Everything this file asserts about a tree asks for it this way. A tree another client
/// changes arrives on its own event, so reading the latest view at an arbitrary instant is
/// asking what the window looked like mid-blink.
fn settled(count: usize) -> Option<Vec<(String, String)>> {
    let root = latest_view()?.regions.into_iter().next()?.root?;
    let panes = leaves(&root);
    let sockets: BTreeSet<&String> = panes.iter().map(|(_, socket)| socket).collect();
    (panes.len() == count && sockets.len() == count && !sockets.contains(&String::new()))
        .then_some(panes)
}

/// The last view the core published, from the callback the shell would register.
///
/// The push direction is the whole point of this message: a daemon-side split reaches the
/// window because the core said so, not because anything asked.
static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);

/// Every agent state the core has pushed, by pane.
///
/// Kept rather than counted, because the question is what the shell was last told a pane's
/// agent is doing - which is what it paints.
static STATES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// The last list of everything the daemons hold, which is what a sidebar row comes from.
static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);

extern "C" fn note_view(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    match Event::decode(bytes) {
        Ok(Event { payload: Some(event::Payload::ViewChanged(view)), .. }) => {
            *VIEW.lock().expect("a panicking reader poisoned the view") = Some(view);
        }
        Ok(Event { payload: Some(event::Payload::PaneStateChanged(state)), .. }) => {
            STATES
                .lock()
                .expect("a panicking reader poisoned the states")
                .push((state.pane_id, state.state));
        }
        Ok(Event { payload: Some(event::Payload::RosterChanged(roster)), .. }) => {
            *ROSTER.lock().expect("a panicking reader poisoned the roster") = Some(roster);
        }
        _ => {}
    }
}

fn latest_view() -> Option<ViewChanged> {
    VIEW.lock().expect("a panicking reader poisoned the view").clone()
}

/// Whether the list holds a row for this pane, and whether it says anything is showing it.
fn listed(pane: &str) -> Option<bool> {
    listed_panes().into_iter().find_map(|(row, on_screen)| (row == pane).then_some(on_screen))
}

/// The one listed pane no region is showing, or nothing while every pane is on screen.
fn hidden_pane() -> Option<String> {
    let hidden: Vec<String> = listed_panes()
        .into_iter()
        .filter_map(|(pane, on_screen)| (!on_screen).then_some(pane))
        .collect();
    match hidden.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// Every listed pane, as Muster's name for it and whether a region is showing it.
/// Every pane the core has heard of, on screen or not, asked of it rather than read off a roster.
fn known_panes() -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => {
            window.panes.into_iter().map(|pane| pane.pane_id).collect()
        }
        other => panic!("asking the core what it holds answered {other:?}"),
    }
}

fn listed_panes() -> Vec<(String, bool)> {
    ROSTER
        .lock()
        .expect("a panicking reader poisoned the roster")
        .as_ref()
        .into_iter()
        .flat_map(|roster| roster.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .map(|row| (row.pane_id.clone(), row.on_screen))
        .collect()
}

/// The last thing the core said about this pane's agent, if it has said anything.
fn latest_state(pane: &str) -> Option<String> {
    STATES
        .lock()
        .expect("a panicking reader poisoned the states")
        .iter()
        .rev()
        .find(|(id, _)| id == pane)
        .map(|(_, state)| state.clone())
}

/// The axis of the split this pane hangs directly off, in the published tree.
///
/// Which axis a pane's own parent has is the only thing that says a split was spelled right
/// *and* aimed right: the number of panes says neither.
fn parent_axis(pane: &str) -> Option<String> {
    fn walk(node: &ViewNode, pane: &str) -> Option<String> {
        let Some(view_node::Node::Split(split)) = &node.node else { return None };
        for child in split.first.iter().chain(split.second.iter()) {
            if let Some(view_node::Node::Pane(leaf)) = &child.node
                && leaf.pane_id == pane
            {
                return Some(split.axis.clone());
            }
            if let Some(found) = walk(child, pane) {
                return Some(found);
            }
        }
        None
    }
    walk(&latest_view()?.regions.into_iter().next()?.root?, pane)
}

/// Every pane in a tree, as (id, link socket path), in reading order.
fn leaves(node: &ViewNode) -> Vec<(String, String)> {
    match &node.node {
        Some(view_node::Node::Pane(pane)) => {
            vec![(pane.pane_id.clone(), pane.link_socket_path.clone())]
        }
        Some(view_node::Node::Split(split)) => {
            split.first.iter().chain(split.second.iter()).flat_map(|child| leaves(child)).collect()
        }
        None => Vec::new(),
    }
}

fn answer(payload: request::Payload) -> Response {
    let request = Request::new(payload);
    Response::decode(muster::dispatch(&request.encode_to_vec()).as_slice())
        .expect("the core answers every request with a decodable response")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => panic!("the core refused: {}", failure.reason),
        None => panic!("the core answered with no payload"),
        // Anything else is the core accepting: what it answers with is the request's business
        // and not this helper's.
        Some(_) => {}
    }
}

/// The reason a request was refused, or a panic saying it was not.
fn refusal(payload: request::Payload) -> String {
    match answer(payload).payload {
        Some(response::Payload::Failure(failure)) => failure.reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn attach(pane: &str) -> muster::proto::Attached {
    match answer(request::Payload::AttachPane(AttachPane { pane_id: pane.to_string() })).payload {
        Some(response::Payload::Attached(attached)) => attached,
        other => panic!("expected an attachment for {pane}, got {other:?}"),
    }
}

/// Every pane the daemon holds, by its own account.
fn panes(daemon: &Daemon) -> Vec<String> {
    snapshot(&mut daemon.connect()).panes.into_iter().map(|pane| pane.pane).collect()
}

/// What a pane is showing, asked of the daemon that renders it.
///
/// A daemon renders every pane whether or not anything is attached to it, which is what
/// makes this a usable oracle here: no surface, no bridge, and a screen to read anyway.
fn screen(daemon: &Daemon, pane: &str) -> String {
    read_text(&mut daemon.connect(), pane, 0, 0).text
}
