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
use muster_core::intent::{BackendChannel, BackendIntent, MoveDestination, Refusal};
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::{PaneId, TabId};
use muster_core::names::{Mint, Names};
use muster_herdr::snapshot::read_snapshot;
use muster_herdr::subscription::Subscription;
use muster_herdr::{HerdrBackend, PaneEnvironment};
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
            to: MoveDestination::Beside { tab: far.clone(), after: elsewhere.clone() },
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

/// A pane pulled into a tab of its own lands in the workspace it came from, not the focused one.
///
/// The measurement the `new_tab` corpus case cites, and the reason Muster sends no
/// `workspace_id`. herdr's other tab-making request, `tab.create`, takes one and drops the tab
/// into whichever workspace the daemon last had focused when it is missing - so a request that
/// named nothing here could as easily have put the tab on the other side of the session, and a
/// one-workspace daemon would never show it.
///
/// Two workspaces, and the pane's own is not the focused one. That is what makes the answer
/// mean something: if herdr read the daemon's cursor, the tab would land in `elsewhere`.
#[test]
fn a_pane_moved_into_a_tab_of_its_own_stays_in_its_own_workspace() {
    let (daemon, mirror, _tab, first, second) = a_tab_of_two();
    let ours = workspace_of(&mirror, &first);

    // A second workspace, focused, so the daemon's own cursor points away from the pane.
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "elsewhere", "focus": true }));
    resnapshot(&daemon, &mirror);

    daemon
        .backend()
        .submit(&BackendIntent::MovePane {
            pane: first.clone(),
            to: MoveDestination::NewTab { name: Some("pulled out".to_string()) },
        })
        .expect("herdr accepts a move into a tab of its own");

    resnapshot(&daemon, &mirror);

    let landed = tab_of(&mirror, &first);
    assert_eq!(
        in_tab(&mirror, &landed),
        vec![first.clone()],
        "the pane is not alone in the tab the move made"
    );
    assert_eq!(
        workspace_of(&mirror, &first),
        ours,
        "the tab landed in the workspace the daemon had focused rather than the pane's own, so \
         Muster has to name a workspace on this request after all"
    );
    assert!(
        !in_tab(&mirror, &tab_of(&mirror, &second)).contains(&first),
        "the pane is still in the tab it came from, so it was copied rather than moved"
    );
    // The name the dance this replaces could not set, because it made the tab before anything
    // knew what was going into it.
    assert_eq!(
        label_of(&mirror, &landed),
        Some("pulled out".to_string()),
        "the new tab did not take the name the move gave it"
    );
}

/// What a tab is called, or nothing when the daemon has not said.
fn label_of(mirror: &Arc<Mutex<Mirror>>, tab: &TabId) -> Option<String> {
    let held = mirror.lock().expect("a panicking test poisoned the mirror");
    held.tab(tab).map(|tab| tab.label.clone())
}

fn workspace_of(mirror: &Arc<Mutex<Mirror>>, pane: &PaneId) -> String {
    let held = mirror.lock().expect("a panicking test poisoned the mirror");
    held.pane(pane).expect("the mirror holds this pane").workspace.to_string()
}

/// A swap across tabs is refused rather than reported as done.
///
/// herdr answers a cross-tab swap with a success carrying `changed: false` and the arrangement
/// it already had, which is the shape nothing above the adapter can tell from a swap that
/// worked - and the reason the core picks the verb from where the two panes are rather than
/// sending a swap and hoping.
///
/// Only a real daemon produces that answer, which is what makes two claims one test: that the
/// adapter refuses it, and that it refuses it as a stale window. `cross_tab` says more than no.
/// `session::arrange_pane` sends a swap only for two panes its mirror has in one tab, so herdr
/// saying they are in two is the daemon stating the window is wrong about one of them.
#[test]
fn a_swap_across_tabs_is_refused_rather_than_reported_as_done() {
    let (daemon, mirror, _tab, first, _second) = a_tab_of_two();
    let home = tab_of(&mirror, &first);
    daemon.call("tab.create", &json!({ "focus": false }));
    resnapshot(&daemon, &mirror);
    let elsewhere = order(&mirror)
        .into_iter()
        .find(|pane| tab_of(&mirror, pane) != home)
        .expect("the new tab brings a pane of its own");

    let refused = daemon
        .backend()
        .submit(&BackendIntent::SwapPanes { pane: first.clone(), with: elsewhere.clone() })
        .expect_err(
            "herdr answered a cross-tab swap as a success, so the adapter passed a change that \
             did not happen off as one that did",
        );
    assert!(
        matches!(refused, Refusal::NotThere(_)),
        "a cross-tab swap is a stale window rather than a request refused on its merits, and \
         only NotThere re-reads the session: {refused:?}"
    );

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
            to: MoveDestination::Beside { tab: far.clone(), after: elsewhere.clone() },
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

/// A pane moved out of a tab it was alone in reaches a listening window, and stays.
///
/// herdr closes the emptied tab and announces that first: `tab_closed` for the tab the pane
/// left, then `pane_moved` for the pane (herdr v0.8.0 `src/app/api/panes.rs`), and the stream
/// writes `tab.closed` ahead of `pane.moved` besides. The mirror takes a tab's panes with it, so
/// for a moment the moved pane is removed - and a mirror that remembered that removal as a close
/// would refuse the move that follows, and lose a pane nobody closed.
#[test]
fn a_pane_moved_out_of_a_tab_it_was_alone_in_is_not_lost() {
    let (daemon, mirror, _tab, first, second) = a_tab_of_two();
    daemon.call("tab.create", &json!({ "focus": false }));
    resnapshot(&daemon, &mirror);
    let alone = order(&mirror)
        .into_iter()
        .find(|pane| pane != &first && pane != &second)
        .expect("the new tab brings a pane of its own");
    let home = tab_of(&mirror, &first);
    let far = tab_of(&mirror, &alone);

    let live = Arc::new(Mutex::new(Mirror::new()));
    let _subscription = Subscription::start(
        daemon.socket_path().to_string_lossy().into_owned(),
        Arc::clone(&live),
        Arc::new(|_| {}),
        daemon.names(),
    );
    until(
        "the subscription to describe the whole session",
        || {
            recorded_in(&live, &home) == sorted([&first, &second])
                && recorded_in(&live, &far) == sorted([&alone])
        },
        (),
    );

    daemon
        .backend()
        .submit(&BackendIntent::MovePane {
            pane: alone.clone(),
            to: MoveDestination::Beside { tab: home.clone(), after: second.clone() },
        })
        .expect("herdr accepts a move into another tab");

    until(
        "the tab it landed in to hold it",
        || recorded_in(&live, &home) == sorted([&first, &second, &alone]),
        || format!("the tab holds {:?}", recorded_in(&live, &home)),
    );
}

/// A pane moved into another workspace keeps its name, and leaves no row behind in the tab it
/// left.
///
/// herdr numbers panes per workspace, so a move across workspaces gives the pane a new id there
/// and says which id it had in `previous_pane_id` (`observations/herdr-0.8.0.md` section 20).
/// The process in the pane was started knowing its name, and a window that names the new id
/// afresh strands that process: its `$MUSTER_PANE` answers to nothing, and the name's old row
/// stays in the tab the pane left (kan a_2P65uM7yI).
///
/// The first workspace keeps a pane of its own, so nothing closes it: a closed workspace takes
/// its rows with it, which would hide a row left behind.
#[test]
fn a_pane_moved_to_another_workspace_keeps_its_name() {
    let session = TwoWorkspaces::watched();
    let TwoWorkspaces { names, live, home, far, first, second, elsewhere, .. } = &session;

    session
        .backend()
        .submit(&BackendIntent::MovePane {
            pane: first.clone(),
            to: MoveDestination::Beside { tab: far.clone(), after: elsewhere.clone() },
        })
        .expect("herdr accepts a move into a tab in another workspace");

    // What the fix rests on, asked of herdr itself: the pane is held under an id it did not have.
    let moved = session
        .held_in(far)
        .into_iter()
        .find(|pane| pane != &session.backend_id(elsewhere))
        .expect("the tab the pane was moved into holds it");
    assert_ne!(
        moved, session.first_id,
        "herdr kept the pane's id across workspaces, so there is no new id for Muster to follow"
    );

    until(
        "the tab it landed in to hold it under the name it was made with",
        || recorded_in(live, far) == sorted([elsewhere, first]),
        || format!("the tab holds {:?}", recorded_in(live, far)),
    );
    until(
        "the tab it left to stop naming it",
        || recorded_in(live, home) == sorted([second]),
        || format!("the tab it left holds {:?}", recorded_in(live, home)),
    );
    assert_eq!(order(live).len(), 3, "the window holds {:?} for three panes", order(live));
    until(
        "the tab's tree to name it the way its record does",
        || in_tab(live, far).contains(first),
        || format!("the tree lays out {:?}", in_tab(live, far)),
    );
    assert_eq!(
        names.backend_pane(first).map(|backend| backend.to_string()),
        Ok(moved),
        "the pane's name does not resolve to the id herdr holds it under now"
    );
}

/// A pane moved out of a workspace it was alone in keeps its name.
///
/// herdr closes the emptied workspace and announces that before the move. The window takes the
/// workspace's rows with it, so for a moment the pane has no row - and the pane then arrives under
/// the name that row had, which must not be refused as a pane already said to be gone.
#[test]
fn a_pane_moved_out_of_a_workspace_it_was_alone_in_keeps_its_name() {
    let session = TwoWorkspaces::watched();
    let TwoWorkspaces { names, live, home, first, second, elsewhere, .. } = &session;

    session
        .backend()
        .submit(&BackendIntent::MovePane {
            pane: elsewhere.clone(),
            to: MoveDestination::Beside { tab: home.clone(), after: second.clone() },
        })
        .expect("herdr accepts a move into a tab in another workspace");

    until(
        "the tab it landed in to hold it under the name it was made with",
        || recorded_in(live, home) == sorted([first, second, elsewhere]),
        || format!("the tab holds {:?}", recorded_in(live, home)),
    );
    assert_eq!(order(live).len(), 3, "the window holds {:?} for three panes", order(live));
    let moved = session
        .held_in(home)
        .into_iter()
        .find(|pane| {
            ![session.first_id.as_str(), session.backend_id(second).as_str()]
                .contains(&pane.as_str())
        })
        .expect("the tab the pane was moved into holds it");
    assert_eq!(
        names.backend_pane(elsewhere).map(|backend| backend.to_string()),
        Ok(moved),
        "the pane's name does not resolve to the id herdr holds it under now"
    );
}

/// Two workspaces - two panes side by side in the first, one in the second - and a window
/// watching them that mints its own names.
///
/// Minted rather than the harness's backend mint, which spells a name as the daemon's own id.
/// Under that mint a pane's name and the id it had before a move are one string, so anything
/// still naming the old id after the move - a replayed layout - takes the name straight back,
/// which a running Muster's names can never do.
///
/// Watched from before the first workspace exists, so every event arrives live and in order.
/// A subscription opened onto an existing session replays it a kind at a time
/// (`observations/herdr-0.8.0.md` section 10), and a replay still arriving after the move names
/// ids the move retired.
struct TwoWorkspaces {
    // Before the daemon, so the subscription is dropped while its daemon is still there.
    _subscription: Subscription,
    daemon: Daemon,
    names: Names,
    live: Arc<Mutex<Mirror>>,
    home: TabId,
    far: TabId,
    first: PaneId,
    first_id: String,
    second: PaneId,
    elsewhere: PaneId,
}

impl TwoWorkspaces {
    fn watched() -> TwoWorkspaces {
        let daemon = Daemon::start();
        let names = Names::alone("local", Mint::Drawn);
        let live = Arc::new(Mutex::new(Mirror::new()));
        let subscription = Subscription::start(
            daemon.socket_path().to_string_lossy().into_owned(),
            Arc::clone(&live),
            Arc::new(|_| {}),
            names.clone(),
        );

        daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "home", "focus": true }));
        let (first_id, home_id) = daemon_panes(&daemon).into_iter().next().expect("a pane");
        daemon.call("pane.split", &json!({ "target_pane_id": first_id, "direction": "right" }));
        daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "far", "focus": false }));
        let panes = daemon_panes(&daemon);
        let second_id = panes
            .iter()
            .find(|(pane, tab)| tab == &home_id && pane != &first_id)
            .map(|(pane, _)| pane.clone())
            .expect("the split made a second pane beside the first");
        let (elsewhere_id, far_id) = panes
            .iter()
            .find(|(_, tab)| tab != &home_id)
            .cloned()
            .expect("the second workspace brings a pane of its own");

        let session = TwoWorkspaces {
            first: names.pane(&first_id),
            second: names.pane(&second_id),
            elsewhere: names.pane(&elsewhere_id),
            home: names.tab(&home_id),
            far: names.tab(&far_id),
            first_id,
            _subscription: subscription,
            daemon,
            names,
            live,
        };
        let TwoWorkspaces { live, home, far, first, second, elsewhere, .. } = &session;
        until(
            "the window to hold both workspaces, trees and all",
            || {
                recorded_in(live, home) == sorted([first, second])
                    && recorded_in(live, far) == sorted([elsewhere])
                    && sorted_tree(live, home) == sorted([first, second])
                    && in_tab(live, far) == vec![elsewhere.clone()]
            },
            || format!("the window holds {:?}", order(live)),
        );
        session
    }

    fn backend(&self) -> HerdrBackend {
        HerdrBackend::new(self.daemon.client(), PaneEnvironment::none(), self.names.clone())
    }

    fn backend_id(&self, pane: &PaneId) -> String {
        self.names.backend_pane(pane).expect("a pane this window named resolves").to_string()
    }

    /// The ids herdr itself holds in one tab, asked of the daemon rather than of any mirror.
    fn held_in(&self, tab: &TabId) -> Vec<String> {
        let tab = self.names.backend_tab(tab).expect("a tab this window named resolves");
        daemon_panes(&self.daemon)
            .into_iter()
            .filter(|(_, holding)| holding == tab.as_str())
            .map(|(pane, _)| pane)
            .collect()
    }
}

/// Every pane the daemon holds, and the tab holding it, in herdr's own ids.
fn daemon_panes(daemon: &Daemon) -> Vec<(String, String)> {
    let listed = daemon.call("pane.list", &json!({}));
    listed["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| {
            Some((pane["pane_id"].as_str()?.to_string(), pane["tab_id"].as_str()?.to_string()))
        })
        .collect()
}

/// One tab's tree, sorted, for a question about membership rather than order.
fn sorted_tree(mirror: &Arc<Mutex<Mirror>>, tab: &TabId) -> Vec<PaneId> {
    let mut panes = in_tab(mirror, tab);
    panes.sort();
    panes
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
    until(
        "the subscription to describe every tab",
        || tab_order(&live) == before,
        || {
            format!(
                "this window holds [{}] where the daemon has [{}], so the reorder below would \
                 have been measured against a session it had not finished reading.",
                spelled(&tab_order(&live)),
                spelled(&before)
            )
        },
    );

    // The last tab to the front, which is the largest move three tabs allow. Asked of the
    // daemon in its own vocabulary because Muster has no intent for it.
    let moving = before.last().expect("three tabs").clone();
    daemon.call("tab.move", &json!({ "tab_id": moving.as_str(), "insert_index": 0 }));

    let expected: Vec<TabId> = std::iter::once(moving.clone())
        .chain(before.iter().filter(|tab| *tab != &moving).cloned())
        .collect();
    until(
        "the listening window to hold the order the daemon settled on",
        || tab_order(&live) == expected,
        || {
            format!(
                "this window holds [{}] and the daemon settled on [{}]. Still [{}] means the \
                 move announced nothing this window asked for, or nothing applied what did \
                 arrive - the two halves this test covers. Any third order means the stated \
                 order was read and then misapplied.",
                spelled(&tab_order(&live)),
                spelled(&expected),
                spelled(&before)
            )
        },
    );
}

/// A tab order for a failure message, spelled the way `observations/herdr-0.8.0.md` spells one.
///
/// Not `{:?}` on the vector, which is the idiom elsewhere in this file: those messages report
/// membership, where a reader only has to see whether a name is present. These report a
/// sequence, and comparing two sequences by eye is the whole of reading one of them.
fn spelled(tabs: &[TabId]) -> String {
    tabs.iter().map(TabId::as_str).collect::<Vec<_>>().join(", ")
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
