//! Dragging a row, against a real herdr.
//!
//! `backend-intent.json` pins the envelope Muster builds and `every_parameter_is_one_herdr
//! _declares` checks its top-level keys against herdr's own schema - but only the top level.
//! The move's `destination` is a nested object, and herdr ignores a key it does not recognise
//! rather than refusing it, so a misspelling inside there is a request that quietly does
//! something else. Against a one-pane daemon that is indistinguishable from working.
//!
//! So these assert the arrangement afterwards. A swap has to exchange two panes and leave the
//! shape alone; a move has to take a pane out of one tab and land it behind a named pane in
//! another. Neither is a claim a recorded case can make, because both are about what a daemon
//! does with a request rather than about what Muster sends.

use std::sync::{Arc, Mutex};

use herdr_harness::{Daemon, until};
use muster_core::intent::{BackendChannel, BackendIntent};
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::{PaneId, TabId};
use muster_herdr::snapshot::read_snapshot;
use muster_herdr::subscription::Subscription;
use serde_json::json;

/// A daemon holding one tab of two panes, side by side.
fn a_tab_of_two() -> (Daemon, Arc<Mutex<Mirror>>, TabId, PaneId, PaneId) {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "arranging", "focus": true }));

    let mirror = Arc::new(Mutex::new(Mirror::new()));
    resnapshot(&daemon, &mirror);
    let first = only_pane(&mirror);
    daemon.call("pane.split", &json!({ "target_pane_id": first.as_str(), "direction": "right" }));
    resnapshot(&daemon, &mirror);

    let order = order(&mirror);
    assert_eq!(
        order.len(),
        2,
        "the split should have made a second pane, and the tab holds {order:?}"
    );
    let tab = tab_of(&mirror, &order[0]);
    let (first, second) = (order[0].clone(), order[1].clone());
    (daemon, mirror, tab, first, second)
}

#[test]
fn a_swap_exchanges_two_panes_and_leaves_the_shape_alone() {
    let (daemon, mirror, tab, first, second) = a_tab_of_two();
    let before = arrangement(&mirror, &tab);

    daemon
        .backend()
        .submit(&BackendIntent::SwapPanes { pane: first.clone(), with: second.clone() })
        .expect("herdr accepts a swap of two panes in one tab");

    resnapshot(&daemon, &mirror);

    // The tab's tree order, which is what the agent list reads. Not the mirror's own pane map,
    // which is keyed by id and would report the same order whatever the swap did.
    assert_eq!(
        in_tab(&mirror, &tab),
        vec![second.clone(), first.clone()],
        "the two panes should have exchanged places in the order the agent list reads"
    );
    // Exchanged, not rebuilt. The whole reason a drag is a swap rather than an insertion is
    // that the arrangement stays put and only the occupants move, so a tab of two side by side
    // is still a tab of two side by side.
    assert_eq!(
        shape(&arrangement(&mirror, &tab)),
        shape(&before),
        "the swap changed the tab's shape rather than only who sits where"
    );
}

#[test]
fn a_move_takes_a_pane_into_another_tab_and_lands_it_behind_the_row_it_was_dropped_on() {
    let (daemon, mirror, _tab, first, second) = a_tab_of_two();

    // A second tab, with a pane of its own, so the move has somewhere to land and something to
    // land behind. Two panes there, because "behind the first" and "at the end" look identical
    // in a tab of one.
    daemon.call("tab.create", &json!({ "focus": false }));
    resnapshot(&daemon, &mirror);
    let elsewhere = order(&mirror)
        .into_iter()
        .find(|pane| pane != &first && pane != &second)
        .expect("the new tab brings a pane of its own");
    let far = tab_of(&mirror, &elsewhere);
    daemon
        .call("pane.split", &json!({ "target_pane_id": elsewhere.as_str(), "direction": "right" }));
    resnapshot(&daemon, &mirror);
    let trailing = in_tab(&mirror, &far)
        .into_iter()
        .find(|pane| pane != &elsewhere)
        .expect("the second tab now holds two panes");

    daemon
        .backend()
        .submit(&BackendIntent::MovePane {
            pane: first.clone(),
            tab: far.clone(),
            after: elsewhere.clone(),
        })
        .expect("herdr accepts a move into another tab");

    resnapshot(&daemon, &mirror);

    // Behind the pane it was dropped on rather than at either end, which is the whole of what
    // the nested `destination` has to get right. Appended would put it after `trailing`, and a
    // `target_pane_id` herdr ignored would look exactly like that.
    assert_eq!(
        in_tab(&mirror, &far),
        vec![elsewhere, first.clone(), trailing],
        "the moved pane did not land immediately behind the one it was dropped on"
    );
    assert!(
        !in_tab(&mirror, &tab_of(&mirror, &second)).contains(&first),
        "the pane is still in the tab it came from, so it was copied rather than moved"
    );
}

#[test]
fn a_swap_across_tabs_does_nothing_and_says_so_only_in_the_log() {
    // Why the gesture needs two verbs rather than one. herdr answers a cross-tab swap with a
    // success carrying `changed: false`, and the layout it carries is the arrangement it
    // already had - so nothing above the adapter can tell this from a swap that worked. That
    // is the whole reason `declined` exists and the reason the core picks the verb from where
    // the two panes are rather than sending a swap and hoping.
    let (daemon, mirror, _tab, first, _second) = a_tab_of_two();
    let home = tab_of(&mirror, &first);
    daemon.call("tab.create", &json!({ "focus": false }));
    resnapshot(&daemon, &mirror);
    let elsewhere = order(&mirror)
        .into_iter()
        .find(|pane| tab_of(&mirror, pane) != home)
        .expect("the new tab brings a pane of its own");

    daemon
        .backend()
        .submit(&BackendIntent::SwapPanes { pane: first.clone(), with: elsewhere.clone() })
        .expect("a declined swap is a success on the wire rather than a transport failure");

    resnapshot(&daemon, &mirror);

    assert_eq!(
        tab_of(&mirror, &first),
        home,
        "the pane crossed tabs, so herdr does perform a cross-tab swap after all and the core \
         could send one verb for both halves of the drag"
    );
}

fn only_pane(mirror: &Arc<Mutex<Mirror>>) -> PaneId {
    let panes = order(mirror);
    assert_eq!(panes.len(), 1, "a fresh workspace holds one pane, and held {panes:?}");
    panes.into_iter().next().expect("just counted one")
}

/// Every pane the mirror holds, in the order it holds them.
fn order(mirror: &Arc<Mutex<Mirror>>) -> Vec<PaneId> {
    mirror.lock().unwrap().panes().map(|pane| pane.id.clone()).collect()
}

/// One tab's panes, in the order its tree lays them out - which is the order a row list reads.
fn in_tab(mirror: &Arc<Mutex<Mirror>>, tab: &TabId) -> Vec<PaneId> {
    let mirror = mirror.lock().unwrap();
    match mirror.layout(tab) {
        Some(layout) => layout.root.panes().into_iter().cloned().collect(),
        None => mirror.panes_in_tab(tab).map(|pane| pane.id.clone()).collect(),
    }
}

fn tab_of(mirror: &Arc<Mutex<Mirror>>, pane: &PaneId) -> TabId {
    mirror
        .lock()
        .unwrap()
        .panes()
        .find(|held| &held.id == pane)
        .unwrap_or_else(|| panic!("{pane} left the mirror"))
        .tab
        .clone()
}

fn arrangement(mirror: &Arc<Mutex<Mirror>>, tab: &TabId) -> String {
    format!("{:?}", mirror.lock().unwrap().layout(tab).map(|layout| layout.root.clone()))
}

/// An arrangement with the pane ids taken out, so two trees can be compared by shape alone.
fn shape(arrangement: &str) -> String {
    arrangement
        .split_whitespace()
        .filter(|word| !word.contains("w1:p"))
        .collect::<Vec<&str>>()
        .join(" ")
}

fn resnapshot(daemon: &Daemon, mirror: &Arc<Mutex<Mirror>>) {
    let fetched = daemon.call("session.snapshot", &json!({}));
    let (snapshot, _dropped) = read_snapshot(
        fetched.get("snapshot").expect("a snapshot with no snapshot in it"),
        &daemon.names(),
    );
    mirror.lock().unwrap().bootstrap(snapshot);
}

#[allow(dead_code)]
#[test]
fn a_move_reaches_a_window_that_is_only_listening() {
    // Every other test in this file re-snapshots, which asks the daemon what it holds and so
    // proves the move happened rather than that a running window would ever see it. This one
    // takes the live route and nothing else: one subscription, and whatever it is told.
    //
    // What it reads is the mirror's own record of which tab holds which pane, NOT the tab's
    // tree. The tree is kept current by `layout_updated` either way, and asserting on it
    // passes whether or not a move was understood - which is the trap this test exists to
    // avoid. The record is what `View::of` compares the tree against, and a tab whose two
    // answers disagree has its tree withheld: so with the move unread, both tabs stop
    // redrawing rather than showing it.
    //
    // Two things had to be true for a window to follow a move and neither was: herdr sends
    // `pane_moved` only to a client that named `pane.moved` in its subscription and Muster's
    // list did not, and the decoder dropped the name anyway.
    let (daemon, mirror, _tab, first, second) = a_tab_of_two();
    daemon.call("tab.create", &json!({ "focus": false }));
    resnapshot(&daemon, &mirror);
    let elsewhere = order(&mirror)
        .into_iter()
        .find(|pane| pane != &first && pane != &second)
        .expect("the new tab brings a pane of its own");
    let far = tab_of(&mirror, &elsewhere);
    let home = tab_of(&mirror, &first);

    // From here the mirror is fed by the subscription alone. Started after the arrangement is
    // built so that the bootstrap describes it and every later change has to arrive as an
    // event.
    let live = Arc::new(Mutex::new(Mirror::new()));
    let _subscription = Subscription::start(
        daemon.socket_path().to_string_lossy().into_owned(),
        Arc::clone(&live),
        Arc::new(|_| {}),
        daemon.names(),
    );
    // The whole arrangement, not a part of it: a bootstrap arrives over several events, and
    // waiting for one tab would start the move against a mirror still filling the other in.
    until(
        "the subscription to describe the whole session",
        || {
            recorded_in(&live, &home) == sorted([&first, &second])
                && recorded_in(&live, &far) == sorted([&elsewhere])
        },
        (),
    );

    daemon
        .backend()
        .submit(&BackendIntent::MovePane {
            pane: first.clone(),
            tab: far.clone(),
            after: elsewhere.clone(),
        })
        .expect("herdr accepts a move into another tab");

    until(
        "the tab it landed in to hold it",
        || recorded_in(&live, &far) == sorted([&elsewhere, &first]),
        (),
    );
    // The tab it left, which is the half easy to forget: a mirror that learned the destination
    // and not the origin still holds a tab whose panes disagree with its tree, and that tab is
    // one that stops redrawing.
    until(
        "the tab it left to stop naming it",
        || recorded_in(&live, &home) == sorted([&second]),
        (),
    );
    assert_eq!(tab_of(&live, &first), far, "the pane's own record still names the old tab");
}

/// A tab reordered by somebody else reaches a window that is only listening.
///
/// The tab half of the test above, and it needs the live route even more than the pane half
/// does: a re-snapshot would prove herdr moved the tab, which was never in doubt. What was in
/// doubt is whether a window hears about it, and the measured answer was no - `tab_moved` goes
/// only to a client that named `tab.moved`, and to one that did not it is not even accompanied
/// by a `layout_updated` (`observations/herdr-0.8.0.md` section 21). So the order changed under
/// a window with nothing at all arriving to say so.
///
/// Muster causes none of these, which is why this drives the daemon directly rather than
/// through an intent. Another client, or herdr's own TUI, is the whole scenario.
#[test]
fn a_tab_reordered_by_somebody_else_reaches_a_window_that_is_only_listening() {
    let (daemon, mirror, _tab, _first, _second) = a_tab_of_two();
    daemon.call("tab.create", &json!({ "focus": false }));
    daemon.call("tab.create", &json!({ "focus": false }));
    resnapshot(&daemon, &mirror);
    let before = tab_order(&mirror);
    assert_eq!(before.len(), 3, "three tabs make an insert distinguishable from an exchange");

    let live = Arc::new(Mutex::new(Mirror::new()));
    let _subscription = Subscription::start(
        daemon.socket_path().to_string_lossy().into_owned(),
        Arc::clone(&live),
        Arc::new(|_| {}),
        daemon.names(),
    );
    until("the subscription to describe every tab", || tab_order(&live) == before);

    // The last tab to the front, which is the largest move three tabs allow. Asked of the
    // daemon in its own vocabulary because Muster has no intent for it.
    let moving = before.last().expect("three tabs").clone();
    daemon.call("tab.move", &json!({ "tab_id": moving.as_str(), "insert_index": 0 }));

    let expected: Vec<TabId> = std::iter::once(moving.clone())
        .chain(before.iter().filter(|tab| *tab != &moving).cloned())
        .collect();
    until("the listening window to hold the order the daemon settled on", || {
        tab_order(&live) == expected
    });
}

/// Every tab the mirror holds, in the order it holds them.
///
/// The order is the whole subject, so this is deliberately not sorted - unlike `recorded_in`
/// below, where membership is the question and an order would be a claim it cannot make.
fn tab_order(mirror: &Arc<Mutex<Mirror>>) -> Vec<TabId> {
    mirror.lock().unwrap().tabs().map(|tab| tab.id.clone()).collect()
}

/// Which panes the mirror records as belonging to a tab, sorted so the comparison is about
/// membership rather than about an order this says nothing about.
///
/// Deliberately not the tab's tree, which is what `in_tab` reads. `View::of` withholds a
/// tree whenever these two disagree, so this is the answer that decides whether a window
/// draws anything at all.
fn sorted<const N: usize>(panes: [&PaneId; N]) -> Vec<PaneId> {
    let mut expected: Vec<PaneId> = panes.into_iter().cloned().collect();
    expected.sort();
    expected
}

fn recorded_in(mirror: &Arc<Mutex<Mirror>>, tab: &TabId) -> Vec<PaneId> {
    let mirror = mirror.lock().unwrap();
    let mut held: Vec<PaneId> = mirror.panes_in_tab(tab).map(|pane| pane.id.clone()).collect();
    held.sort();
    held
}
