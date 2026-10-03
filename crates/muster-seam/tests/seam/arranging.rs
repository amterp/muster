//! Dragging a row, through the seam, against a real daemon.
//!
//! The gesture the suite could describe from every angle and never perform. Its pieces are
//! each covered: `SidebarTests` decides which drops are legal, and the daemon client's own
//! tests pin the requests it builds. Between the protobuf a window sends and the roster a window draws there was
//! nothing, so nothing could be wrong about it - which is the shape of every bug this tier
//! was added for.
//!
//! So this sends the bytes a shell sends and reads the bytes a shell renders. `ArrangePane`
//! in, `RosterChanged` out, a real daemon behind it, and no daemon verb named anywhere in the
//! test: which of a swap and a move a drop becomes is the core's decision and is exactly what
//! would go unnoticed.
//!
//! Both destinations a move has are here, because both are decided on this side of the seam:
//! whether a drop becomes a swap or a move is read off where the two panes are, and whether the
//! tab a move makes comes on screen is the window's own answer rather than the daemon's.

use std::sync::Mutex;

use muster::proto::{
    ArrangePane, Event, OpenWindow, Request, Response, RosterChanged, Startup, ViewNode, event,
    request, response, view_node,
};
use muster_daemon_proto::Side;
use muster_harness::requests::{beside, create, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

/// A row dropped on another row in the same tab exchanges the two panes, and dropping it again
/// puts them back - read off the roster the daemon's own tree produces.
#[test]
fn a_row_dropped_on_another_moves_the_pane_it_names() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();

    muster::ffi::muster_set_event_callback(Some(note_roster));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

    until(
        "the window to list the two panes it opened onto",
        || rows().len() == 2,
        || format!("the roster holds {:?}", rows()),
    );
    let before = rows();
    let (first, second) = (before[0].clone(), before[1].clone());
    // Named rather than left empty. A drag knows which machine it happened on, and the seam
    // refuses a move that does not say.
    let machine = daemon_id();

    // The drop: the first row onto the second. Same tab, so the two exchange places - and
    // what makes this worth running is that the order comes back from the daemon's own tree
    // rather than from anything Muster arranged, so a request that reached the wrong pane
    // reads here as a list that did not move.
    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        daemon_id: machine.clone(),
        pane_id: first.clone(),
        onto_pane_id: second.clone(),
        ..ArrangePane::default()
    })));

    until(
        "the two rows to exchange places",
        || rows() == vec![second.clone(), first.clone()],
        || format!("the roster holds {:?}, and started as {before:?}", rows()),
    );

    // Both panes still listed, once each. An exchange that lost one, or that grew a third
    // from an echo applied twice, would satisfy an assertion about the first row alone.
    let after = rows();
    let mut sorted = after.clone();
    sorted.sort();
    let mut expected = before.clone();
    expected.sort();
    assert_eq!(sorted, expected, "the drag changed which panes exist, not only their order");

    // And back, because an exchange that works one way is one nobody can undo - and because
    // the second drop starts from the arrangement the first produced rather than from the one
    // the daemon opened with.
    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        daemon_id: machine,
        pane_id: first.clone(),
        onto_pane_id: second.clone(),
        ..ArrangePane::default()
    })));
    until(
        "the rows to go back",
        || rows() == before,
        || format!("the roster holds {:?}, and should have returned to {before:?}", rows()),
    );
}

/// Pulling a pane out of a split is one request, and it costs no pane.
///
/// The dance this replaces made a tab, moved into it, and closed the pane the first command had
/// started - a login shell on this machine and an ssh session on another, opened and killed
/// seconds apart, with the keyboard passing through it on the way (kan `a_2IXGSgZi7`).
///
/// Counting the panes before and afterwards is what says so. A tab holding one pane is the same
/// picture whether nothing extra was made or something was made and thrown away, and only the
/// count tells the two apart.
#[test]
fn a_pane_pulled_into_a_tab_of_its_own_costs_no_pane_and_no_keyboard() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();

    muster::ffi::muster_set_event_callback(Some(note_roster));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to list the two panes it opened onto",
        || rows().len() == 2 && tabs().len() == 1,
        || format!("the roster holds {:?} in {:?}", rows(), tabs()),
    );
    let before = rows();
    let keyboard_was = keyboard();
    // A pane the keyboard is not on, which is the case worth pinning: an agent pulling another
    // agent's pane out of a split must not lose its own place doing it. Pulling the pane the
    // keyboard is on moves it either way, because the pane it was on has left the region.
    let pulled = before
        .iter()
        .find(|pane| Some((*pane).clone()) != keyboard_was)
        .expect("the window opened onto more than one pane")
        .clone();

    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        daemon_id: daemon_id(),
        pane_id: pulled.clone(),
        new_tab: true,
        tab_name: "pulled out".to_string(),
        ..ArrangePane::default()
    })));

    until(
        "the pane to be alone in a tab of its own",
        || tabs().len() == 2 && rows_in_tab_of(&pulled) == vec![pulled.clone()],
        || format!("the roster holds {:?} in {:?}", rows(), tabs()),
    );

    let mut after = rows();
    after.sort();
    let mut expected = before.clone();
    expected.sort();
    assert_eq!(
        after, expected,
        "the move changed which panes exist. A pane made and closed on the way is what this \
         command exists to stop costing."
    );
    assert_eq!(
        keyboard(),
        keyboard_was,
        "pulling somebody else's pane out of a split moved the keyboard, which is what the \
         dance this replaces did by way of the throwaway pane it opened"
    );
    // And the window did not change what it is showing. Bringing the new tab on screen would
    // put the tab somebody is working in behind it, which is the same interruption arriving a
    // different way.
    assert!(
        on_screen().contains(&keyboard_was.clone().expect("the keyboard is on a pane")),
        "the tab the keyboard is in went off screen, so the window followed the tab the move \
         made"
    );
    assert!(
        tabs().iter().any(|(_, label)| label.contains("pulled out")),
        "the new tab did not take the name the move gave it: {:?}",
        tabs()
    );
}

/// A pane dropped on its neighbor's bottom edge goes below it: the side by side pair becomes
/// one above the other, which no move could do before a move could name a side.
///
/// Read off the tree the view carries rather than the roster, because the roster lists panes in
/// reading order and `[p1 | p2]` and `[p2 / p1]` read the same.
#[test]
fn a_pane_moved_below_its_neighbor_turns_the_split_on_its_side() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    assert_eq!(shape(), "columns(p1,p2)");

    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: "p1".to_string(),
        onto_pane_id: "p2".to_string(),
        side: "down".to_string(),
        ..ArrangePane::default()
    })));
    until(
        "the pair to stand one above the other",
        || shape() == "rows(p2,p1)",
        || format!("the tab's tree is {}", shape()),
    );

    // And to the left of it, which puts the pair back side by side the other way round.
    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: "p1".to_string(),
        onto_pane_id: "p2".to_string(),
        side: "left".to_string(),
        ..ArrangePane::default()
    })));
    until(
        "the pair to stand side by side again",
        || shape() == "columns(p1,p2)",
        || format!("the tab's tree is {}", shape()),
    );
}

/// A side reaches into another tab too: the pane leaves its own and lands on that side of the
/// pane named, where without a side it would only have landed after it.
#[test]
fn a_pane_moved_beside_one_in_another_tab_lands_on_that_side() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: "p1".to_string(),
        new_tab: true,
        ..ArrangePane::default()
    })));
    until(
        "p1 to have a tab of its own",
        || rows_in_tab_of("p1") == ["p1"],
        || format!("the tabs hold {:?}", tabs()),
    );

    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: "p1".to_string(),
        onto_pane_id: "p2".to_string(),
        side: "up".to_string(),
        ..ArrangePane::default()
    })));
    until(
        "p1 to be back in p2's tab, above it",
        || rows_in_tab_of("p2") == ["p1", "p2"] && tabs().len() == 1,
        || format!("the tabs hold {:?}", tabs()),
    );
    assert_eq!(shape(), "rows(p1,p2)");
}

/// A side that is not one of the four is refused, and so is a side with nothing to be beside.
#[test]
fn a_side_has_to_be_one_of_four_and_of_a_pane() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);

    for refused in [
        ArrangePane {
            pane_id: "p1".to_string(),
            onto_pane_id: "p2".to_string(),
            side: "sideways".to_string(),
            ..ArrangePane::default()
        },
        ArrangePane {
            pane_id: "p1".to_string(),
            new_tab: true,
            side: "down".to_string(),
            ..ArrangePane::default()
        },
    ] {
        match answer(request::Payload::ArrangePane(refused.clone())).payload {
            Some(response::Payload::Failure(failure)) => assert!(!failure.reason.is_empty()),
            other => panic!("{refused:?} answered {other:?}"),
        }
    }
    assert_eq!(shape(), "columns(p1,p2)", "a refused move rearranged the tab");
}

fn open_window(daemon: &Daemon) {
    muster::ffi::muster_set_event_callback(Some(note_roster));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to show the two panes it opened onto",
        || shape() == "columns(p1,p2)",
        || format!("the tab's tree is {}", shape()),
    );
}

/// The tree of the region the keyboard is in, written out: `columns(p1,p2)` is p1 left of p2,
/// `rows(p2,p1)` is p2 above p1.
fn shape() -> String {
    fn written(node: &ViewNode) -> String {
        match &node.node {
            Some(view_node::Node::Pane(pane)) => pane.pane_id.clone(),
            Some(view_node::Node::Split(split)) => format!(
                "{}({},{})",
                split.axis,
                split.first.as_deref().map(written).unwrap_or_default(),
                split.second.as_deref().map(written).unwrap_or_default()
            ),
            None => String::new(),
        }
    }
    match answer(request::Payload::ReadWindow(muster::proto::ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => window
            .view
            .and_then(|view| {
                let region = view
                    .regions
                    .into_iter()
                    .find(|region| region.region_id == view.focused_region)?;
                region.root.as_ref().map(written)
            })
            .unwrap_or_default(),
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

/// A daemon holding one tab of two panes, `p1` on the left and `p2` on the right.
fn two_panes_side_by_side() -> Daemon {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", Side::Right)));
    daemon
}

static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);

extern "C" fn note_roster(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::RosterChanged(roster)) = event.payload {
        *ROSTER.lock().expect("a panicking test poisoned the roster") = Some(roster);
    }
}

/// Every pane the roster lists, in the order it lists them.
///
/// The list a person reads down, flattened across daemons and tabs the way the sidebar draws
/// it - which is the order `cmd+1` to `cmd+9` count in, so it is the answer the gesture is
/// about rather than a convenience.
fn rows() -> Vec<String> {
    ROSTER
        .lock()
        .expect("a panicking test poisoned the roster")
        .iter()
        .flat_map(|roster| &roster.tabs)
        .flat_map(|tab| &tab.panes)
        .map(|pane| pane.pane_id.clone())
        .collect()
}

/// Every tab the roster lists, with what it is called.
fn tabs() -> Vec<(String, String)> {
    ROSTER
        .lock()
        .expect("a panicking test poisoned the roster")
        .iter()
        .flat_map(|roster| &roster.tabs)
        .map(|tab| (tab.tab_id.clone(), tab.label.clone()))
        .collect()
}

/// The panes sharing a tab with this one, in the order the roster lists them.
fn rows_in_tab_of(pane: &str) -> Vec<String> {
    ROSTER
        .lock()
        .expect("a panicking test poisoned the roster")
        .iter()
        .flat_map(|roster| &roster.tabs)
        .filter(|tab| tab.panes.iter().any(|held| held.pane_id == pane))
        .flat_map(|tab| &tab.panes)
        .map(|held| held.pane_id.clone())
        .collect()
}

/// Every pane the window says it is drawing.
fn on_screen() -> Vec<String> {
    ROSTER
        .lock()
        .expect("a panicking test poisoned the roster")
        .iter()
        .flat_map(|roster| &roster.tabs)
        .flat_map(|tab| &tab.panes)
        .filter(|pane| pane.on_screen)
        .map(|pane| pane.pane_id.clone())
        .collect()
}

/// Which pane the window's keyboard is on, as the roster's own marking has no answer for.
///
/// Read off the view rather than the roster: the roster says what exists and the view says
/// where the keyboard is, and this test is about the second.
fn keyboard() -> Option<String> {
    match answer(request::Payload::ReadWindow(muster::proto::ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => {
            let view = window.view?;
            let region =
                view.regions.iter().find(|region| region.region_id == view.focused_region)?;
            Some(region.pane_id.clone()).filter(|pane| !pane.is_empty())
        }
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

/// The daemon the roster says these panes are on.
fn daemon_id() -> String {
    ROSTER
        .lock()
        .expect("a panicking test poisoned the roster")
        .iter()
        .flat_map(|roster| &roster.tabs)
        .flat_map(|tab| &tab.daemon_ids)
        .next()
        .cloned()
        .expect("the roster names no daemon at all")
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
