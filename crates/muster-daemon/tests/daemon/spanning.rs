//! A tab whose tree holds panes on other machines (MIP-8): what a window may write, what this
//! daemon still changes on its own, and what survives a restart and a handoff.

use crate::persistence::{state_file, stop, until_restored, until_saved};
use crate::support::*;
use proto::{Side, pane_request, placement, tab_request};

/// A tab `t1` holding `[a|b]`, on a subscribed connection.
fn two_panes() -> (Daemon, Control) {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    make(&mut control, create("a", in_new_tab("t1")));
    make(&mut control, create("b", beside("a", Side::Right)));
    (daemon, control)
}

/// `t1` with a devenv pane `x` under `a`, as a window writes it.
const SPANNING: &str = "[[a/~x@devenv 0.50]|b 0.50]";

fn spanning() -> (Daemon, Control) {
    let (daemon, mut control) = two_panes();
    expect(&mut control, set_tree_request("t1", SPANNING, 1, 0), proto::Outcome::Done);
    (daemon, control)
}

fn layout_generation(control: &mut Control, tab: &str) -> u64 {
    let snapshot = snapshot(control);
    snapshot.tabs.iter().find(|held| held.tab == tab).expect("the tab is here").layout_generation
}

fn moved(name: &str, placement: proto::Placement) -> proto::request::Service {
    pane(pane_request::Request::Move(pane_request::Move {
        pane: name.to_string(),
        placement: Some(placement),
    }))
}

#[test]
fn a_window_writes_a_tree_holding_another_machines_pane() {
    let (_daemon, mut control) = two_panes();
    let asked = expect(&mut control, set_tree_request("t1", SPANNING, 1, 0), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_changed:t1"]);
    assert_eq!(tab_shape(&mut control, "t1"), SPANNING);
    assert_eq!(layout_generation(&mut control, "t1"), 1);
    expect(&mut control, set_tree_request("t1", SPANNING, 1, 0), proto::Outcome::AlreadySo);
}

#[test]
fn a_tree_that_would_lose_or_invent_a_pane_or_undo_a_newer_one_is_refused() {
    let (_daemon, mut control) = spanning();
    make(&mut control, create("c", in_new_tab("t2")));
    let refused = [
        ("[[a/~x@devenv 0.50]|b 0.50]", 2, 0, "worked out from an older tree"),
        ("[a|~x@devenv 0.50]", 2, 1, "leaves out b"),
        ("[[a/a 0.50]|b 0.50]", 2, 1, "names a twice"),
        ("[[a/z 0.50]|b 0.50]", 2, 1, "names z, which this daemon does not hold"),
        ("[[a/c 0.50]|b 0.50]", 2, 1, "names c, which is in tab t2"),
        ("[a|b 0.50]", 1, 1, "a generation not above its base"),
    ];
    for (tree, generation, based_on, why) in refused {
        let asked = control.ask(set_tree_request("t1", tree, generation, based_on));
        assert_eq!(asked.outcome(), proto::Outcome::Refused, "{why}: {}", asked.answer.reason);
    }
    assert_eq!(tab_shape(&mut control, "t1"), SPANNING, "nothing refused changed the tab");

    let zoomed = |on: &str| {
        tab(tab_request::Request::SetTree(tab_request::SetTree {
            tab: "t1".to_string(),
            root: Some(tree(SPANNING)),
            zoomed: Some(on.to_string()),
            generation: 2,
            based_on: 1,
        }))
    };
    expect(&mut control, zoomed("nowhere"), proto::Outcome::Refused);
    expect(&mut control, zoomed("x"), proto::Outcome::Done);
    let snapshot = snapshot(&mut control);
    let t1 = snapshot.tabs.iter().find(|held| held.tab == "t1").unwrap();
    assert_eq!(
        t1.zoomed.as_deref(),
        Some("x"),
        "a zoom may fill the tab with another machine's pane"
    );
}

#[test]
fn a_spanning_tab_is_rearranged_only_by_a_window_writing_its_tree() {
    let (_daemon, mut control) = spanning();
    make(&mut control, create("c", in_new_tab("t2")));
    make(&mut control, create("d", beside("c", Side::Right)));
    let resize = |name: &str| {
        pane(pane_request::Request::Resize(pane_request::Resize {
            pane: name.to_string(),
            direction: Side::Right.into(),
            fraction: Some(0.1),
        }))
    };
    let zoom = |name: &str| {
        pane(pane_request::Request::Zoom(pane_request::Zoom {
            pane: name.to_string(),
            zoomed: true,
        }))
    };
    let swap = |one: &str, with: &str| {
        pane(pane_request::Request::Swap(pane_request::Swap {
            pane: one.to_string(),
            with: with.to_string(),
        }))
    };
    let ratio = |tab: &str| {
        self::tab(tab_request::Request::SetSplitRatio(tab_request::SetSplitRatio {
            tab: tab.to_string(),
            path: Vec::new(),
            ratio: 0.3,
        }))
    };
    for (change, plain) in
        [(resize("a"), resize("c")), (zoom("a"), zoom("c")), (swap("a", "b"), swap("c", "d"))]
    {
        let asked = control.ask(change);
        assert_eq!(asked.outcome(), proto::Outcome::Refused, "{}", asked.answer.reason);
        assert!(asked.answer.reason.contains("spans machines"), "{}", asked.answer.reason);
        expect(&mut control, plain, proto::Outcome::Done);
    }
    expect(&mut control, ratio("t1"), proto::Outcome::Refused);
    expect(&mut control, ratio("t2"), proto::Outcome::Done);
    assert_eq!(tab_shape(&mut control, "t1"), SPANNING);
}

#[test]
fn a_pane_made_in_a_tree_goes_where_the_tree_says() {
    let (_daemon, mut control) = spanning();
    let into = "[[a/~x@devenv 0.50]|[b/c 0.50] 0.50]";
    let asked = make(&mut control, create("c", in_tree("t1", into, 2, 1)));
    assert!(names(&asked.events).contains(&"tab_changed:t1".to_string()));
    assert_eq!(tab_shape(&mut control, "t1"), into);
    assert_eq!(layout_generation(&mut control, "t1"), 2);

    let stale = control.ask(create_request(create("d", in_tree("t1", into, 2, 1))));
    assert_eq!(stale.outcome(), proto::Outcome::Refused, "{}", stale.answer.reason);
}

#[test]
fn a_pane_made_in_a_tree_on_a_machine_with_no_part_of_the_tab_makes_one() {
    let (_daemon, mut control) = two_panes();
    let asked = make(&mut control, create("d", in_tree("t9", "[~y@devenv|d 0.50]", 3, 2)));
    assert!(names(&asked.events).contains(&"tab_opened:t9".to_string()));
    assert_eq!(tab_shape(&mut control, "t9"), "[~y@devenv|d 0.50]");
    assert_eq!(layout_generation(&mut control, "t9"), 3);

    let hidden = proto::Placement {
        r#where: Some(placement::Where::InTree(placement::InTree {
            tab: "t8".to_string(),
            root: Some(tree("[~y@devenv|e 0.50]")),
            generation: 1,
            based_on: 0,
            label: Some(proto::Label { text: Some("work".to_string()), generation: 4 }),
            hidden: true,
        })),
    };
    make(&mut control, create("e", hidden));
    let snapshot = snapshot(&mut control);
    let t8 = snapshot.tabs.iter().find(|held| held.tab == "t8").unwrap();
    assert!(t8.hidden, "a part made for a hidden tab is hidden too");
    assert_eq!(t8.label.as_ref().and_then(|label| label.text.as_deref()), Some("work"));
}

#[test]
fn a_pane_moved_in_a_tree_keeps_this_daemons_part_of_the_tab_open() {
    let (_daemon, mut control) = spanning();
    let rearranged = "[[a/b 0.50]|~x@devenv 0.50]";
    let asked =
        expect(&mut control, moved("b", in_tree("t1", rearranged, 2, 1)), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_changed:t1"], "no part closed and opened again");
    assert_eq!(tab_shape(&mut control, "t1"), rearranged);

    make(&mut control, create("c", in_new_tab("t2")));
    let joined = "[[a/b 0.50]|[~x@devenv/c 0.50] 0.50]";
    let asked = expect(&mut control, moved("c", in_tree("t1", joined, 3, 2)), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_closed:t2", "tab_changed:t1"]);
    assert_eq!(tab_shape(&mut control, "t1"), joined);
}

#[test]
fn a_pane_closing_keeps_every_foreign_leaf_and_the_last_one_here_closes_the_part() {
    let (_daemon, mut control) = spanning();
    let asked = expect(&mut control, close_request("b"), proto::Outcome::Done);
    assert!(names(&asked.events).contains(&"tab_changed:t1".to_string()));
    assert_eq!(tab_shape(&mut control, "t1"), "[a/~x@devenv 0.50]");
    assert_eq!(layout_generation(&mut control, "t1"), 1, "a pane closing is not a window's write");

    let asked = expect(&mut control, close_request("a"), proto::Outcome::Done);
    assert!(names(&asked.events).contains(&"tab_closed:t1".to_string()));
    assert!(snapshot(&mut control).tabs.is_empty(), "the devenv keeps the tab; nothing runs here");
}

#[test]
fn a_restart_brings_back_every_foreign_leaf_in_its_place() {
    let (mut daemon, mut control) = spanning();
    until_saved(&daemon, "\"foreign\"");
    assert!(written(&state_file(&daemon)).contains("\"version\": 2"));
    stop(&mut daemon, &mut control);
    daemon.restart();

    let mut control = daemon.connect();
    until_restored(&mut control, 2);
    assert_eq!(tab_shape(&mut control, "t1"), SPANNING);
    assert_eq!(layout_generation(&mut control, "t1"), 1);
}

#[test]
fn a_file_with_no_tab_spanning_machines_is_still_written_as_version_1() {
    let (daemon, _control) = two_panes();
    until_saved(&daemon, "\"t1\"");
    assert!(written(&state_file(&daemon)).contains("\"version\": 1"));
}

#[test]
fn a_handoff_carries_every_foreign_leaf() {
    let (mut daemon, mut control) = spanning();
    let before = snapshot(&mut control).tabs;
    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    let mut control = daemon.connect();
    assert_eq!(snapshot(&mut control).tabs, before);
}
