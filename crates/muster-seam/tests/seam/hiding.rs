//! Hiding a pane and making one quiet, through the seam, against a real daemon.
//!
//! Both are the daemon's facts - a tab made hidden, a pane flagged quiet - and everything a window
//! does about them is decided on this side: which window holds a hidden tab (none), what the
//! roster lists it under, where a pane brought back lands, and what a quiet pane may ask. So this
//! sends what a shell or the CLI sends and reads what a shell renders.

use std::sync::Mutex;

use muster::proto::{
    AttentionChanged, ClosePane, Event, FocusHistory, FocusPane, HideHelpers, HidePane, OpenWindow,
    QuietPane, ReadWindow, RenamePane, RenameTab, Request, Response, RosterChanged, ShowHelpers,
    ShowPane, SplitPane, Startup, event, request, response,
};
use muster_daemon_proto::{Side, input_event};
use muster_harness::requests::{beside, create, in_new_tab, make, snapshot, until_text};
use muster_harness::{Daemon, Input, until};
use prost::Message;

/// Hiding takes a pane out of the window and into the hidden list, held by its daemon; showing
/// brings it back into a tab of its own with the keyboard on it.
#[test]
fn a_hidden_pane_leaves_the_window_and_comes_back_in_a_tab_of_its_own() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    let keyboard_was = keyboard();

    assert_ok(&answer(request::Payload::HidePane(HidePane {
        pane_id: "p2".to_string(),
        ..HidePane::default()
    })));
    until(
        "p2 to leave the window for the hidden list",
        || rows() == ["p1"] && hidden() == ["p2"],
        || format!("the window lists {:?} and hides {:?}", rows(), hidden()),
    );
    let mut control = daemon.connect();
    let held = snapshot(&mut control);
    let tab = held.tabs.iter().find(|tab| tab_holds(tab, "p2")).expect("p2 is in a tab");
    assert!(tab.hidden, "the daemon does not hold p2's tab as hidden: {tab:?}");
    assert_eq!(keyboard(), keyboard_was, "hiding another pane moved the keyboard");

    // Hiding it again changes nothing, and says so as a success.
    assert_ok(&answer(request::Payload::HidePane(HidePane {
        pane_id: "p2".to_string(),
        ..HidePane::default()
    })));

    assert_ok(&answer(request::Payload::ShowPane(ShowPane {
        pane_id: "p2".to_string(),
        take_focus: true,
        ..ShowPane::default()
    })));
    until(
        "p2 to come back in a tab of its own, with the keyboard",
        || {
            hidden().is_empty()
                && tabs() == 2
                && rows_in_tab_of("p2") == ["p2"]
                && keyboard().as_deref() == Some("p2")
        },
        || {
            format!(
                "the window lists {:?} in {} tabs, hides {:?}, and has the keyboard on {:?}",
                rows(),
                tabs(),
                hidden(),
                keyboard()
            )
        },
    );
}

/// A hidden pane keeps the word its daemon drew, and is brought back by it: nothing shows its id
/// while it is hidden except the list, and the word is the name that list leads with.
#[test]
fn a_hidden_pane_is_shown_by_its_word() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    let word = snapshot(&mut daemon.connect())
        .panes
        .into_iter()
        .find(|pane| pane.pane == "p2")
        .and_then(|pane| pane.word)
        .expect("p2 was drawn a word");
    assert_ok(&answer(request::Payload::HidePane(HidePane {
        pane_id: word.clone(),
        ..HidePane::default()
    })));
    until("p2 to be hidden", || hidden() == ["p2"], || format!("hidden: {:?}", hidden()));

    assert_ok(&answer(request::Payload::ShowPane(ShowPane {
        pane_id: word,
        take_focus: true,
        ..ShowPane::default()
    })));
    until(
        "p2 to be back, with the keyboard",
        || hidden().is_empty() && keyboard().as_deref() == Some("p2"),
        || format!("hidden: {:?}, keyboard on {:?}", hidden(), keyboard()),
    );
}

/// Going to a hidden pane - a banner, the chord to whatever asks, a click on its row - brings it
/// back first, since nothing can show it where it is.
#[test]
fn focusing_a_hidden_pane_brings_it_back() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    assert_ok(&answer(request::Payload::HidePane(HidePane {
        pane_id: "p2".to_string(),
        ..HidePane::default()
    })));
    until("p2 to be hidden", || hidden() == ["p2"], || format!("hidden: {:?}", hidden()));

    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        pane_id: "p2".to_string(),
        ..FocusPane::default()
    })));
    until(
        "p2 to be back, with the keyboard",
        || hidden().is_empty() && keyboard().as_deref() == Some("p2"),
        || format!("hidden: {:?}, keyboard on {:?}", hidden(), keyboard()),
    );
}

/// Hiding the only pane a window shows leaves it a fresh tab rather than a blank window - even
/// in a window whose machine already gave it its first tab, which closing a last pane would not
/// ask again.
#[test]
fn hiding_a_windows_last_pane_opens_a_fresh_tab() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    open_window(&daemon);
    let first = keyboard().expect("an empty machine's window opened onto a tab of its own");

    assert_ok(&answer(request::Payload::HidePane(HidePane {
        pane_id: first.clone(),
        ..HidePane::default()
    })));
    until(
        "the window to open a fresh tab beside the hidden pane",
        || {
            hidden() == [first.as_str()]
                && tabs() == 1
                && keyboard().is_some_and(|pane| pane != first)
                && rows().len() == 1
        },
        || {
            format!(
                "the window lists {:?} in {} tabs, hides {:?}, and has the keyboard on {:?}",
                rows(),
                tabs(),
                hidden(),
                keyboard()
            )
        },
    );
}

/// Back steps over a pane hidden since the keyboard left it, rather than failing on it or bringing
/// it back: hiding it said not to show it, and back is only a way around the window.
#[test]
fn back_steps_over_a_pane_hidden_since() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    let start = keyboard().expect("the window opened onto a pane");
    let other = if start == "p1" { "p2" } else { "p1" };
    for pane in [other, start.as_str()] {
        assert_ok(&answer(request::Payload::FocusPane(FocusPane {
            pane_id: pane.to_string(),
            ..FocusPane::default()
        })));
        until(
            "the keyboard to move",
            || keyboard().as_deref() == Some(pane),
            || format!("the keyboard is on {:?}", keyboard()),
        );
    }
    assert_ok(&answer(request::Payload::HidePane(HidePane {
        pane_id: other.to_string(),
        ..HidePane::default()
    })));
    until("the pane to be hidden", || hidden() == [other], || format!("hidden: {:?}", hidden()));

    match answer(request::Payload::FocusHistory(FocusHistory { forward: false })).payload {
        Some(response::Payload::Went(went)) => {
            assert_eq!(went.pane_id, start, "back did not step over the hidden pane");
        }
        other => panic!("going back answered {other:?}"),
    }
    assert_eq!(hidden(), [other], "going back brought the hidden pane back");
}

/// A pane made hidden and quiet from the start is the headless agent: no window takes its tab,
/// the hidden list says it is quiet, and it closes by name though nothing shows it.
#[test]
fn a_pane_made_hidden_is_taken_by_no_window_and_closes_by_name() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    let made = match answer(request::Payload::SplitPane(SplitPane {
        pane_id: "p1".to_string(),
        side: "right".to_string(),
        hidden: true,
        quiet: true,
        ..SplitPane::default()
    }))
    .payload
    {
        Some(response::Payload::Made(made)) => made.pane_id,
        other => panic!("making a hidden pane answered {other:?}"),
    };
    until(
        "the new pane to be listed hidden and quiet",
        || hidden() == [made.clone()] && quiet_hidden() == [made.clone()],
        || format!("hidden: {:?}, quiet among them: {:?}", hidden(), quiet_hidden()),
    );
    assert_eq!(rows(), ["p1", "p2"], "the window's own tabs changed");

    assert_ok(&answer(request::Payload::ClosePane(ClosePane {
        pane_id: made.clone(),
        ..ClosePane::default()
    })));
    until("the hidden pane to close", || hidden().is_empty(), || format!("{:?}", hidden()));

    // Hidden and taking the keyboard is a contradiction, and is refused rather than half-done.
    let refused = answer(request::Payload::SplitPane(SplitPane {
        pane_id: "p1".to_string(),
        side: "right".to_string(),
        hidden: true,
        take_focus: true,
        ..SplitPane::default()
    }));
    assert!(
        matches!(refused.payload, Some(response::Payload::Failure(_))),
        "a hidden pane taking the keyboard answered {refused:?}"
    );
}

/// A pane's hidden helpers share one hidden tab, named for it by the name it goes by, and a helper
/// shown beside it is split there as any pane is. Every row says whose helper it is, by its
/// maker's id though the request named the maker by its word.
#[test]
fn a_panes_helpers_share_a_hidden_tab_named_for_it() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    let word = snapshot(&mut daemon.connect())
        .panes
        .into_iter()
        .find(|pane| pane.pane == "p1")
        .and_then(|pane| pane.word)
        .expect("p1 was drawn a word");
    let helper = |hidden: bool| {
        let made = answer(request::Payload::SplitPane(SplitPane {
            pane_id: "p1".to_string(),
            side: "down".to_string(),
            hidden,
            maker: word.clone(),
            ..SplitPane::default()
        }));
        match made.payload {
            Some(response::Payload::Made(made)) => made.pane_id,
            other => panic!("making a helper answered {other:?}"),
        }
    };
    let (first, second) = (helper(true), helper(true));
    until(
        "both helpers to be listed hidden, in one tab",
        || roster().hidden.len() == 1 && hidden() == [first.clone(), second.clone()],
        || format!("hidden: {:?}", roster().hidden),
    );
    let shared = &roster().hidden[0];
    assert_eq!(shared.label, format!("{word}'s helpers"));
    assert!(shared.panes.iter().all(|pane| pane.maker == "p1"), "{shared:?}");

    let shown = helper(false);
    until(
        "the shown helper to be split beside its maker",
        || rows() == ["p1", shown.as_str(), "p2"],
        || format!("the window lists {:?}", rows()),
    );
    let makers: Vec<String> =
        roster().tabs.iter().flat_map(|tab| &tab.panes).map(|pane| pane.maker.clone()).collect();
    assert_eq!(makers, ["", "p1", ""], "only the helper has a maker");

    let refused = answer(request::Payload::SplitPane(SplitPane {
        pane_id: "p1".to_string(),
        side: "right".to_string(),
        maker: "p9nobody".to_string(),
        ..SplitPane::default()
    }));
    match refused.payload {
        Some(response::Payload::Failure(failure)) => {
            assert!(failure.reason.contains("p9nobody"), "{}", failure.reason);
        }
        other => panic!("a helper of a pane nobody holds answered {other:?}"),
    }
}

/// A maker's hidden helpers are listed under it, come forward together into one tab after the
/// window's others, arranged as they were and named after the maker as it is called now, and go
/// back together. A tab somebody renamed keeps its name, and one holding a pane that is not a
/// helper stays on screen.
#[test]
fn a_makers_helpers_come_forward_together_and_go_back() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    let helper = |pane: &str, maker: &str| {
        let made = answer(request::Payload::SplitPane(SplitPane {
            pane_id: pane.to_string(),
            side: "right".to_string(),
            hidden: !maker.is_empty(),
            maker: maker.to_string(),
            ..SplitPane::default()
        }));
        match made.payload {
            Some(response::Payload::Made(made)) => made.pane_id,
            other => panic!("making a pane answered {other:?}"),
        }
    };
    let (first, second) = (helper("p1", "p1"), helper("p1", "p1"));
    until(
        "both helpers to be listed under p1",
        || {
            hidden() == [first.clone(), second.clone()]
                && roster().hidden[0].panes.iter().all(|pane| pane.under == "p1")
        },
        || format!("hidden: {:?}", roster().hidden),
    );
    assert_ok(&answer(request::Payload::RenamePane(RenamePane {
        pane_id: "p1".to_string(),
        name: "integrator".to_string(),
        ..RenamePane::default()
    })));
    let show = || {
        answer(request::Payload::ShowHelpers(ShowHelpers {
            pane_id: "integrator".to_string(),
            ..ShowHelpers::default()
        }))
    };
    let hide = || {
        answer(request::Payload::HideHelpers(HideHelpers {
            pane_id: "p1".to_string(),
            ..HideHelpers::default()
        }))
    };
    let last_tab = || roster().tabs.last().cloned().unwrap_or_default();

    assert_ok(&show());
    until(
        "the helpers to come forward as one tab after the window's others",
        || {
            hidden().is_empty()
                && tabs() == 2
                && rows_in_tab_of(&first) == [first.clone(), second.clone()]
                && last_tab().label == "integrator's helpers"
        },
        || format!("tabs: {:?}, hidden: {:?}", roster().tabs, hidden()),
    );
    assert_eq!(rows()[..2], ["p1".to_string(), "p2".to_string()], "the window's own tab first");

    assert_ok(&hide());
    until(
        "the helpers to go back together",
        || tabs() == 1 && hidden() == [first.clone(), second.clone()],
        || format!("tabs: {:?}, hidden: {:?}", roster().tabs, hidden()),
    );
    assert_ok(&hide());

    // A tab somebody renamed keeps the name they gave it.
    assert_ok(&show());
    until("the helpers to come forward again", || tabs() == 2, || format!("{:?}", roster().tabs));
    assert_ok(&answer(request::Payload::RenameTab(RenameTab {
        tab_id: last_tab().tab_id,
        name: "mine".to_string(),
        ..RenameTab::default()
    })));
    until("the rename to land", || last_tab().label == "mine", || format!("{:?}", last_tab()));
    assert_ok(&hide());
    until("the helpers to go back", || tabs() == 1, || format!("{:?}", roster().tabs));
    assert_ok(&show());
    until("the helpers to come forward named as they were", || tabs() == 2, String::new);
    assert_eq!(last_tab().label, "mine");

    // With a pane in it that is nobody's helper, the tab is somebody's own, and stays.
    let own = helper(&first, "");
    until(
        "the pane to join the helpers' tab",
        || rows_in_tab_of(&own).len() == 3,
        || format!("{:?}", roster().tabs),
    );
    assert_ok(&hide());
    assert_eq!(tabs(), 2, "a tab holding a pane that is not a helper was hidden");

    let refused = answer(request::Payload::ShowHelpers(ShowHelpers {
        pane_id: "p2".to_string(),
        ..ShowHelpers::default()
    }));
    match refused.payload {
        Some(response::Payload::Failure(failure)) => {
            assert!(
                failure.reason.starts_with("no machine this window follows holds a helper of p2"),
                "{}",
                failure.reason
            );
        }
        other => panic!("showing the helpers of a pane with none answered {other:?}"),
    }
}

/// A maker closing with a helper still running tells the person once, and the banner the window
/// is asked for carries Muster's words rather than a count from `@muster`.
#[test]
fn a_closed_makers_notice_carries_its_words_to_the_banner() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    let made = answer(request::Payload::SplitPane(SplitPane {
        pane_id: "p2".to_string(),
        side: "right".to_string(),
        hidden: true,
        maker: "p2".to_string(),
        ..SplitPane::default()
    }));
    assert!(matches!(made.payload, Some(response::Payload::Made(_))), "{made:?}");
    assert_ok(&answer(request::Payload::ClosePane(ClosePane {
        pane_id: "p2".to_string(),
        ..ClosePane::default()
    })));
    until(
        "the window to be asked for a banner saying p2 closed",
        || asked().iter().any(|asked| !asked.group.is_empty() && asked.said.contains("closed;")),
        || format!("asked: {:?}", asked()),
    );
    let banner = asked().into_iter().find(|asked| !asked.group.is_empty()).expect("just seen");
    assert!(banner.said.contains("--helpers-of p2"), "{banner:?}");
}

/// A quiet pane asks for nobody: a program's notification there raises nothing, where the same
/// notification in a pane that is not quiet does.
#[test]
fn a_quiet_pane_asks_for_nobody() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", in_new_tab("t2")));
    make(&mut control, create("p3", in_new_tab("t3")));
    until_text(&mut control, "p3", "$");
    open_window(&daemon);
    assert_ok(&answer(request::Payload::QuietPane(QuietPane {
        pane_id: "p2".to_string(),
        quiet: Some(true),
        ..QuietPane::default()
    })));
    until("p2 to be quiet", || quiet_rows() == ["p2"], || format!("{:?}", quiet_rows()));

    // Neither p2 nor p3 is the pane on screen, so each would ask if it could. p3 goes second, and
    // its asking is what says the notification from p2 has been and gone.
    let mut input = Input::connect(daemon.socket_path());
    for pane in ["p2", "p3"] {
        let send = input_event::Send {
            text: r"printf '\033]9;look here\a'".to_string(),
            enter: true,
            ..Default::default()
        };
        input.send(pane, input_event::Input::Send(send));
    }
    until(
        "p3's notification to ask",
        || asked().iter().any(|asked| asked.pane_id == "p3" && asked.state == "notified"),
        || format!("the window asked {:?}", asked()),
    );
    assert!(
        !asked().iter().any(|asked| asked.pane_id == "p2" && asked.state != "withdrawn"),
        "the quiet pane asked: {:?}",
        asked()
    );

    // Toggled, as a menu item sends it, it notifies again.
    assert_ok(&answer(request::Payload::QuietPane(QuietPane {
        pane_id: "p2".to_string(),
        ..QuietPane::default()
    })));
    until("p2 to notify again", || quiet_rows().is_empty(), || format!("{:?}", quiet_rows()));
}

fn two_panes_side_by_side() -> Daemon {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", Side::Right)));
    daemon
}

fn open_window(daemon: &Daemon) {
    ROSTER.lock().expect("a panicking test poisoned the roster").take();
    ASKED.lock().expect("a panicking test poisoned the log").clear();
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to list its panes",
        || !rows().is_empty() && keyboard().is_some(),
        || format!("the roster holds {:?}", rows()),
    );
}

fn tab_holds(tab: &muster_daemon_proto::Tab, pane: &str) -> bool {
    fn holds(node: &muster_daemon_proto::Node, pane: &str) -> bool {
        match node.node.as_ref() {
            Some(muster_daemon_proto::node::Node::Pane(held)) => held == pane,
            Some(muster_daemon_proto::node::Node::Split(split)) => {
                split.first.as_ref().is_some_and(|first| holds(first, pane))
                    || split.second.as_ref().is_some_and(|second| holds(second, pane))
            }
            None => false,
        }
    }
    tab.root.as_ref().is_some_and(|root| holds(root, pane))
}

static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);
static ASKED: Mutex<Vec<AttentionChanged>> = Mutex::new(Vec::new());

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::RosterChanged(roster)) => {
            *ROSTER.lock().expect("a panicking test poisoned the roster") = Some(roster);
        }
        Some(event::Payload::AttentionChanged(asked)) => {
            ASKED.lock().expect("a panicking test poisoned the log").push(asked);
        }
        _ => {}
    }
}

fn roster() -> RosterChanged {
    ROSTER.lock().expect("a panicking test poisoned the roster").clone().unwrap_or_default()
}

/// Every pane in the window's own tabs, in the order the roster lists them.
fn rows() -> Vec<String> {
    roster().tabs.iter().flat_map(|tab| &tab.panes).map(|pane| pane.pane_id.clone()).collect()
}

fn tabs() -> usize {
    roster().tabs.len()
}

fn rows_in_tab_of(pane: &str) -> Vec<String> {
    roster()
        .tabs
        .iter()
        .filter(|tab| tab.panes.iter().any(|held| held.pane_id == pane))
        .flat_map(|tab| &tab.panes)
        .map(|held| held.pane_id.clone())
        .collect()
}

/// Every pane the roster lists as hidden.
fn hidden() -> Vec<String> {
    roster().hidden.iter().flat_map(|tab| &tab.panes).map(|pane| pane.pane_id.clone()).collect()
}

fn quiet_hidden() -> Vec<String> {
    let roster = roster();
    roster
        .hidden
        .iter()
        .flat_map(|tab| &tab.panes)
        .filter(|pane| pane.quiet)
        .map(|pane| pane.pane_id.clone())
        .collect()
}

fn quiet_rows() -> Vec<String> {
    let roster = roster();
    roster
        .tabs
        .iter()
        .flat_map(|tab| &tab.panes)
        .filter(|pane| pane.quiet)
        .map(|pane| pane.pane_id.clone())
        .collect()
}

fn asked() -> Vec<AttentionChanged> {
    ASKED.lock().expect("a panicking test poisoned the log").clone()
}

/// Which pane the window's keyboard is on, read off the view.
fn keyboard() -> Option<String> {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => {
            let view = window.view?;
            let region =
                view.regions.iter().find(|region| region.region_id == view.focused_region)?;
            Some(region.pane_id.clone()).filter(|pane| !pane.is_empty())
        }
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Opened(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
