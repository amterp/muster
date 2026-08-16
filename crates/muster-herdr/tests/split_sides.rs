//! Splitting toward a side herdr has no request for, against a real herdr.
//!
//! What only a live daemon can answer: not whether the arrangement ends up right - the
//! conformance cases pin the envelopes and `mirror.json` pins the suppression - but whether a
//! window ever *renders* the arrangement in between. herdr publishes both, 100.4 ms apart
//! (`observations/herdr-0.8.0.md` section 14), and a stand-in daemon publishing one would be
//! Muster's own guess at herdr agreeing with Muster (`docs/testing.md`).
//!
//! So these watch the mirror rather than read it once. An assertion at the end passes whether
//! the wrong arrangement was never there or was there for six frames and left.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use herdr_harness::Daemon;
use muster_core::intent::{BackendChannel, BackendIntent, Side};
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::{PaneId, TabId};
use muster_herdr::subscription::Subscription;
use serde_json::json;

/// Long enough to be past herdr's own second publish, which was measured at 100.4 ms.
const PAST_THE_SECOND_PUBLISH: Duration = Duration::from_millis(500);

/// Every distinct arrangement one tab was seen holding, in the order it held them.
///
/// Polled rather than reported, because `Change::LayoutChanged` names the tab and not the
/// tree - and the tree is the whole question here.
struct Watcher {
    seen: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Watcher {
    fn on(mirror: &Arc<Mutex<Mirror>>, tab: TabId) -> Watcher {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (mirror, held, halt) = (Arc::clone(mirror), Arc::clone(&seen), Arc::clone(&stop));
        thread::spawn(move || {
            while !halt.load(Ordering::Relaxed) {
                let now = mirror
                    .lock()
                    .unwrap()
                    .layout(&tab)
                    .map_or_else(|| "(none)".to_string(), |layout| layout.root.to_string());
                let mut held = held.lock().unwrap();
                if held.last() != Some(&now) {
                    held.push(now);
                }
                drop(held);
                thread::sleep(Duration::from_millis(1));
            }
        });
        Watcher { seen, stop }
    }

    fn arrangements(&self) -> Vec<String> {
        self.stop.store(true, Ordering::Relaxed);
        self.seen.lock().unwrap().clone()
    }
}

fn session() -> (Daemon, Arc<Mutex<Mirror>>, Subscription, TabId, PaneId) {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));

    let mirror = Arc::new(Mutex::new(Mirror::new()));
    let subscription = Subscription::start(
        daemon.socket_path().to_string_lossy().into_owned(),
        Arc::clone(&mirror),
        Arc::new(|_| {}),
    );
    until("the first pane to reach the mirror", || mirror.lock().unwrap().panes().count() == 1);

    let (tab, pane) = {
        let held = mirror.lock().unwrap();
        let pane = held.panes().next().expect("a workspace comes with a pane");
        (pane.tab.clone(), pane.id.clone())
    };
    (daemon, mirror, subscription, tab, pane)
}

fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out after 10s waiting for {what}");
}

#[test]
fn splitting_leftward_never_shows_the_pane_on_the_right() {
    let (daemon, mirror, _subscription, tab, pane) = session();
    let client = daemon.backend();
    let watcher = Watcher::on(&mirror, tab.clone());

    let outcome = client
        .submit(&BackendIntent::SplitPane {
            pane: pane.clone(),
            side: Side::Left,
            ratio: None,
            cwd: Some("/tmp".into()),
        })
        .expect("a real daemon refused a split it can do");
    let created = outcome.created.clone().expect("pane.split did not name the pane it made");
    let settled = outcome.settled.clone().expect("pane.swap did not answer with a layout");
    mirror.lock().unwrap().settle(settled);

    // Past herdr's own second publish, so that a mirror which merely got there first and then
    // walked backwards fails here rather than passing on timing.
    until("the new pane to reach the mirror", || mirror.lock().unwrap().panes().count() == 2);
    thread::sleep(PAST_THE_SECOND_PUBLISH);

    let leftward = format!("columns({created}, {pane}@0.5)");
    let rightward = format!("columns({pane}, {created}@0.5)");
    let arrangements = watcher.arrangements();

    assert_eq!(
        mirror.lock().unwrap().layout(&tab).map(|layout| layout.root.to_string()),
        Some(leftward.clone()),
        "the tab did not settle on the arrangement that was asked for"
    );
    assert!(
        !arrangements.contains(&rightward),
        "the window showed the new pane on the right on its way to the left, which is the \
         whole defect: {arrangements:?}"
    );
    assert!(
        arrangements.contains(&leftward),
        "the leftward arrangement was never on screen at all: {arrangements:?}"
    );
}

#[test]
fn splitting_rightward_still_costs_one_request_and_no_suppression() {
    // The control. Two of the four sides are a split and nothing else, and an adapter that
    // started swapping on all of them would pass every test above while doubling the work.
    let (daemon, mirror, _subscription, tab, pane) = session();
    let client = daemon.backend();

    let outcome = client
        .submit(&BackendIntent::SplitPane {
            pane: pane.clone(),
            side: Side::Right,
            ratio: None,
            cwd: Some("/tmp".into()),
        })
        .expect("a real daemon refused a split it can do");
    let created = outcome.created.clone().expect("pane.split did not name the pane it made");
    assert!(
        outcome.settled.is_none(),
        "a rightward split answered with an arrangement, so it asked herdr twice"
    );

    until("the tree to reach the mirror", || {
        mirror.lock().unwrap().layout(&tab).is_some_and(|layout| layout.root.panes().len() == 2)
    });
    assert_eq!(
        mirror.lock().unwrap().layout(&tab).map(|layout| layout.root.to_string()),
        Some(format!("columns({pane}, {created}@0.5)"))
    );
}

#[test]
fn resizes_faster_than_the_daemon_announces_them_do_not_walk_backwards() {
    // A resize chord held down: each repeat is answered before the previous one is broadcast,
    // so a mirror that took the answers and then applied every broadcast would jump the
    // divider back one step per event. herdr answers `pane.resize` with the settled layout for
    // the same reason it answers a swap with one.
    let (daemon, mirror, _subscription, tab, pane) = session();
    let client = daemon.backend();
    client
        .submit(&BackendIntent::SplitPane {
            pane: pane.clone(),
            side: Side::Right,
            ratio: None,
            cwd: Some("/tmp".into()),
        })
        .expect("a real daemon refused a split it can do");
    until("the tree to reach the mirror", || {
        mirror.lock().unwrap().layout(&tab).is_some_and(|layout| layout.root.panes().len() == 2)
    });

    let watcher = Watcher::on(&mirror, tab.clone());
    for _ in 0..5 {
        let outcome = client
            .submit(&BackendIntent::ResizePane {
                pane: pane.clone(),
                direction: Side::Right,
                amount: Some(2.0),
            })
            .expect("a real daemon refused a resize it can do");
        if let Some(settled) = outcome.settled {
            mirror.lock().unwrap().settle(settled);
        }
    }
    thread::sleep(PAST_THE_SECOND_PUBLISH);

    let ratios: Vec<f32> = watcher
        .arrangements()
        .iter()
        .filter_map(|tree| tree.rsplit_once('@')?.1.trim_end_matches(')').parse().ok())
        .collect();
    assert!(ratios.len() > 1, "nothing about the divider was observed moving: {ratios:?}");
    assert!(
        ratios.windows(2).all(|pair| pair[0] <= pair[1]),
        "the divider moved back toward where it came from, which is an answer being \
         overtaken by the broadcast it superseded: {ratios:?}"
    );
}

#[test]
fn closing_a_pane_a_leftward_split_made_collapses_the_tab() {
    // The shape that got past the corpus and showed up in the running app: a tab settled by an
    // answer, then collapsed back to the arrangement it had before. herdr broadcasts that
    // collapse and it is the only thing saying the pane is gone - suppressing it leaves a dead
    // square on screen that nothing can dismiss, because the tree the view is withholding
    // still names a pane the tab no longer holds.
    let (daemon, mirror, _subscription, tab, pane) = session();
    let client = daemon.backend();

    let outcome = client
        .submit(&BackendIntent::SplitPane {
            pane: pane.clone(),
            side: Side::Up,
            ratio: None,
            cwd: Some("/tmp".into()),
        })
        .expect("a real daemon refused a split it can do");
    let created = outcome.created.clone().expect("pane.split did not name the pane it made");
    mirror.lock().unwrap().settle(outcome.settled.clone().expect("no layout from the swap"));
    until("the new pane to reach the mirror", || mirror.lock().unwrap().panes().count() == 2);

    client
        .submit(&BackendIntent::ClosePane { pane: created.clone() })
        .expect("a real daemon refused a close it can do");

    until("the tab to collapse back to one pane", || {
        mirror.lock().unwrap().layout(&tab).is_some_and(|layout| layout.root.panes().len() == 1)
    });
    assert_eq!(
        mirror.lock().unwrap().layout(&tab).map(|layout| layout.root.to_string()),
        Some(pane.to_string()),
        "the tab kept a tree naming a pane the daemon no longer holds"
    );
}

#[test]
fn a_dragged_divider_lands_on_the_answer_rather_than_the_broadcast() {
    // A drag, which is the same shape as the held resize above and arrives faster: one request
    // per mouse-moved event, each answered long before the previous one is broadcast. The
    // difference is that herdr states this arrangement as its exported tree rather than as the
    // flat rectangles every other layout uses, so until there was a reader for that shape the
    // answer read as "no arrangement stated" and every drag fell back to the event stream -
    // where about ten frames' worth of positions from a hundred milliseconds ago each triggered
    // another relayout while the pointer had moved on. That is the shaking.
    let (daemon, mirror, _subscription, tab, pane) = session();
    let client = daemon.backend();
    client
        .submit(&BackendIntent::SplitPane {
            pane: pane.clone(),
            side: Side::Right,
            ratio: None,
            cwd: Some("/tmp".into()),
        })
        .expect("a real daemon refused a split it can do");
    until("the tree to reach the mirror", || {
        mirror.lock().unwrap().layout(&tab).is_some_and(|layout| layout.root.panes().len() == 2)
    });

    let watcher = Watcher::on(&mirror, tab.clone());
    // A pointer crossing the pane, at the granularity a drag actually produces - and entirely
    // to one side of where the divider was resting, so that every position the watcher can
    // catch is further along the gesture than the one before it. A drag that started on the
    // other side would make the first legitimate step look like the backwards jump this is
    // watching for, and whether the watcher caught that step at all is a race.
    let dragged: Vec<f32> = (6..=9u8).map(|step| f32::from(step) / 10.0).collect();
    for ratio in &dragged {
        let outcome = client
            .submit(&BackendIntent::SetSplitRatio {
                tab: tab.clone(),
                path: Vec::new(),
                ratio: *ratio,
            })
            .expect("a real daemon refused a ratio it can set");
        let settled = outcome.settled.expect(
            "layout.set_split_ratio answered with no arrangement, so a drag is back to waiting \
             for the broadcast. herdr states this one as its exported tree - check that \
             read_exported_layout still reads what the daemon publishes",
        );
        mirror.lock().unwrap().settle(settled);
    }
    thread::sleep(PAST_THE_SECOND_PUBLISH);

    let (settled, other) = {
        let held = mirror.lock().unwrap();
        let other = held
            .panes()
            .map(|held| held.id.clone())
            .find(|id| *id != pane)
            .expect("the split gave this tab a second pane");
        (held.layout(&tab).map(|layout| layout.root.to_string()), other)
    };
    assert_eq!(
        settled,
        Some(format!("columns({pane}, {other}@0.9)")),
        "the divider did not come to rest where the drag left it. Landing back at the ratio \
         the drag started from is the superseded bound overflowing: every arrangement between \
         an answer and its broadcast has to be remembered, and a drag produces about ten of \
         them"
    );

    let ratios: Vec<f32> = watcher
        .arrangements()
        .iter()
        .filter_map(|tree| tree.rsplit_once('@')?.1.trim_end_matches(')').parse().ok())
        .collect();
    assert!(ratios.len() > 1, "nothing about the divider was observed moving: {ratios:?}");
    assert!(
        ratios.windows(2).all(|pair| pair[0] <= pair[1]),
        "the divider moved back toward where it came from mid-drag, which is a stale broadcast \
         overtaking the answer that superseded it: {ratios:?}"
    );
}
