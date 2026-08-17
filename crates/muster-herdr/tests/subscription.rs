//! What only a real daemon can answer: does the connection dial, notice a hang-up, back
//! off, and rebuild rather than resume.
//!
//! Everything decidable about the stream is judged offline in `backend_events.rs` and
//! `backend_snapshot.rs`, from recorded bytes. What is left is behavior that only exists
//! when there is a socket at the other end, and a fake socket would only be Muster's guess
//! at herdr - so these run against the pinned binary (`docs/testing.md`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use herdr_harness::Daemon;
use muster_core::AgentState;
use muster_core::mirror::backend::{Health, LayoutNode, PaneId, SplitAxis, TabId};
use muster_core::mirror::{Change, Mirror};
use muster_core::names::{Mint, Names};
use muster_herdr::subscription::{Notice, Subscription};
use serde_json::{Value, json};

/// Everything a test wants to look at afterwards, written from the subscription's thread.
#[derive(Debug, Default)]
struct Log {
    notices: Mutex<Vec<Notice>>,
}

impl Log {
    fn record(self: &Arc<Log>) -> muster_herdr::subscription::Report {
        let log = Arc::clone(self);
        Arc::new(move |notice| log.notices.lock().unwrap().push(notice))
    }

    fn notices(&self) -> Vec<Notice> {
        self.notices.lock().unwrap().clone()
    }

    fn bootstraps(&self) -> usize {
        self.notices().iter().filter(|n| matches!(n, Notice::Bootstrapped { .. })).count()
    }
}

/// Waits for a condition, or fails saying what it was still waiting for.
///
/// Polling rather than sleeping a fixed time: herdr answers in under a millisecond, so a
/// sleep long enough to be safe is long enough to make the suite unpleasant, and one short
/// enough to be pleasant is flaky on a loaded machine.
fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out after 10s waiting for {what}");
}

fn mirror_and_log(daemon: &Daemon) -> (Arc<Mutex<Mirror>>, Arc<Log>, Subscription) {
    let mirror = Arc::new(Mutex::new(Mirror::new()));
    let log = Arc::new(Log::default());
    let subscription = Subscription::start(
        daemon.socket_path().to_string_lossy().into_owned(),
        Arc::clone(&mirror),
        log.record(),
        daemon.names(),
    );
    (mirror, log, subscription)
}

fn pane_count(mirror: &Arc<Mutex<Mirror>>) -> usize {
    mirror.lock().unwrap().panes().count()
}

#[test]
fn a_subscription_mirrors_a_session_it_arrived_after() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "before", "focus": true }));

    let (mirror, log, _subscription) = mirror_and_log(&daemon);
    until("the first bootstrap", || log.bootstraps() > 0);

    let mirror = mirror.lock().unwrap();
    assert_eq!(mirror.health(), Health::Connected);
    assert_eq!(mirror.workspaces().count(), 1);
    assert_eq!(mirror.panes().count(), 1);
    // The pane herdr created with the workspace, focused. Read rather than assumed,
    // because a focus cursor that never arrives is exactly the bug this catches.
    assert!(mirror.focus().pane.is_some(), "no focused pane after bootstrap");
}

#[test]
fn the_replayed_session_does_not_double_the_one_just_snapshotted() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));

    let (mirror, log, _subscription) = mirror_and_log(&daemon);
    until("the first bootstrap", || log.bootstraps() > 0);
    // The replay lands after the snapshot and describes the same session. Nothing marks
    // where it ends, so this waits out the window in which it would have arrived.
    std::thread::sleep(Duration::from_millis(300));

    assert_eq!(pane_count(&mirror), 1, "the replay was applied as a second session");
    let doubled = log
        .notices()
        .iter()
        .filter(|notice| matches!(notice, Notice::Changed(Change::PaneAdded(_))))
        .count();
    assert_eq!(doubled, 0, "the replay reported creations for panes the snapshot already had");
}

#[test]
fn a_pane_created_after_the_subscription_arrives_on_it() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));

    let (mirror, log, _subscription) = mirror_and_log(&daemon);
    until("the first bootstrap", || log.bootstraps() > 0);

    daemon.call("pane.split", &json!({ "direction": "right" }));
    until("the new pane to reach the mirror", || pane_count(&mirror) == 2);
}

#[test]
fn a_closed_pane_leaves_the_mirror() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));
    let split = daemon.call("pane.split", &json!({ "direction": "right" }));
    let new_pane = split
        .get("pane")
        .and_then(|pane| pane.get("pane_id"))
        .and_then(|id| id.as_str())
        .expect("pane.split did not name the pane it made")
        .to_string();

    let (mirror, log, _subscription) = mirror_and_log(&daemon);
    until("the first bootstrap", || log.bootstraps() > 0);
    assert_eq!(pane_count(&mirror), 2);

    daemon.call("pane.close", &json!({ "pane_id": new_pane }));
    until("the closed pane to leave the mirror", || pane_count(&mirror) == 1);
}

#[test]
fn a_dead_daemon_makes_the_mirror_stale_without_emptying_it() {
    let mut daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));

    let (mirror, log, _subscription) = mirror_and_log(&daemon);
    until("the first bootstrap", || log.bootstraps() > 0);

    daemon.kill();
    until("the mirror to go stale", || mirror.lock().unwrap().health() == Health::Stale);

    // The point of stale rather than empty: the last thing the daemon said is still the
    // best answer available, and a window that blanks on a dropped connection has thrown
    // away everything the user was looking at.
    assert_eq!(pane_count(&mirror), 1, "a dropped connection emptied the mirror");
}

#[test]
fn an_absent_daemon_is_disconnected_rather_than_stale() {
    let mirror = Arc::new(Mutex::new(Mirror::new()));
    let log = Arc::new(Log::default());
    let _subscription = Subscription::start(
        "/tmp/muster-test/nothing-here.sock",
        Arc::clone(&mirror),
        log.record(),
        Names::alone("local", Mint::Backend),
    );

    until("the failed dial to be reported", || !log.notices().is_empty());
    // Never connected, so there is no last good answer to describe as aging. The
    // distinction is what the window's chrome says to a user who has not started a daemon
    // versus one whose daemon just died.
    assert_eq!(mirror.lock().unwrap().health(), Health::Disconnected);
    assert_eq!(log.bootstraps(), 0);
}

#[test]
fn dropping_the_handle_stops_the_thread() {
    let daemon = Daemon::start();
    let alive = Arc::new(AtomicBool::new(true));
    let mirror = Arc::new(Mutex::new(Mirror::new()));

    {
        let log = Arc::new(Log::default());
        let _subscription = Subscription::start(
            daemon.socket_path().to_string_lossy().into_owned(),
            Arc::clone(&mirror),
            log.record(),
            daemon.names(),
        );
        until("the first bootstrap", || log.bootstraps() > 0);
        alive.store(false, Ordering::Relaxed);
    }

    // Nothing to assert but the absence of a hang: a subscription that outlives its handle
    // holds a connection open and keeps writing into a mirror the window has forgotten,
    // and the symptom is a daemon that will not shut down.
    std::thread::sleep(Duration::from_millis(200));
    assert!(!alive.load(Ordering::Relaxed));
}

#[test]
fn an_agent_state_survives_a_replayed_pane() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));
    let panes = daemon.call("session.snapshot", &json!({}));
    let pane_id = panes["snapshot"]["panes"][0]["pane_id"]
        .as_str()
        .expect("the fresh session has no pane")
        .to_string();

    daemon.call(
        "pane.report_agent",
        &json!({ "pane_id": pane_id, "agent": "probe", "source": "probe", "state": "working" }),
    );

    let (mirror, log, _subscription) = mirror_and_log(&daemon);
    until("the first bootstrap", || log.bootstraps() > 0);
    // Long enough for the replay to land, since it is the replay that would roll this
    // back: herdr's pane payloads carry agent_status as of when the subscription opened.
    std::thread::sleep(Duration::from_millis(300));

    let mirror = mirror.lock().unwrap();
    let state = mirror.agent_state(&PaneId::new(pane_id)).expect("the pane left the mirror");
    assert_eq!(
        state.as_str(),
        "working",
        "a structure event rolled back the agent state, which is the one thing this \
         product exists to show"
    );
}

#[test]
fn every_pane_reports_its_agent_state_without_being_attached_to() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));
    daemon.call("pane.split", &json!({ "direction": "right" }));

    let (mirror, log, _subscription) = mirror_and_log(&daemon);
    until("both panes to reach the mirror", || pane_count(&mirror) == 2);

    let panes: Vec<String> =
        mirror.lock().unwrap().panes().map(|pane| pane.id.to_string()).collect();
    assert_eq!(panes.len(), 2);

    // The second pane, deliberately: an implementation that watches only what the window
    // is attached to would pass on the first and fail here, which is the whole difference
    // between a terminal and the thing this is meant to be.
    let elsewhere = &panes[1];
    daemon.call(
        "pane.report_agent",
        &json!({ "pane_id": elsewhere, "agent": "probe", "source": "probe", "state": "blocked" }),
    );

    until("the unattached pane's state to arrive", || {
        mirror.lock().unwrap().agent_state(&PaneId::new(elsewhere.clone()))
            == Some(AgentState::Blocked)
    });
    assert!(log.notices().iter().any(|notice| matches!(
        notice,
        Notice::Changed(Change::AgentStateChanged { to, .. })
            if *to == AgentState::Blocked
    )));
}

#[test]
fn a_pane_created_later_is_watched_too() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));

    let (mirror, log, _subscription) = mirror_and_log(&daemon);
    until("the first bootstrap", || log.bootstraps() > 0);

    // Split after the subscription is up, so the watcher for this pane can only exist if
    // the set follows the mirror rather than being decided once at connect.
    let split = daemon.call("pane.split", &json!({ "direction": "down" }));
    let new_pane = split["pane"]["pane_id"].as_str().expect("no pane id").to_string();
    until("the new pane to reach the mirror", || pane_count(&mirror) == 2);

    daemon.call(
        "pane.report_agent",
        &json!({ "pane_id": new_pane, "agent": "probe", "source": "probe", "state": "working" }),
    );
    until("the new pane's agent state to arrive", || {
        mirror.lock().unwrap().agent_state(&PaneId::new(new_pane.clone()))
            == Some(AgentState::Working)
    });
}

#[test]
fn a_tab_arranged_by_a_real_daemon_arrives_as_a_tree() {
    // The recorded corpus judges the translation and this judges the plumbing: that the
    // subscription asks for `layout.updated`, that what comes back is where the decoder
    // looks for it, and that a tree built from a live daemon's rectangles is the tree that
    // daemon says it has. All three were right in the recording by construction, because
    // the recording is where they came from.
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));

    let (mirror, log, _subscription) = mirror_and_log(&daemon);
    until("the first bootstrap", || log.bootstraps() > 0);

    let tab = TabId::new(
        daemon.call("session.snapshot", &json!({}))["snapshot"]["focused_tab_id"]
            .as_str()
            .expect("a focused tab")
            .to_string(),
    );
    let single = mirror.lock().unwrap().layout(&tab).expect("the bootstrap carried a tree").clone();
    assert!(matches!(single.root, LayoutNode::Pane(_)), "one pane is not a split: {single:?}");

    // Split after the subscription is up, so what arrives can only have come from the
    // event rather than from the snapshot that bootstrapped it.
    daemon.call("pane.split", &json!({ "direction": "right" }));
    until("the split to reach the mirror as a tree", || {
        mirror
            .lock()
            .unwrap()
            .layout(&tab)
            .is_some_and(|layout| matches!(layout.root, LayoutNode::Split { .. }))
    });

    let mirror = mirror.lock().unwrap();
    let layout = mirror.layout(&tab).expect("the tab still has a tree");
    // Against the daemon's own exported tree rather than against a shape written here: if
    // both were wrong in the same way, an assertion written from memory would agree with
    // them.
    let exported = daemon.call("layout.export", &json!({}));
    let root = &exported["layout"]["root"];
    assert_eq!(root["type"], "split", "herdr does not think this tab is split: {exported}");
    assert_eq!(
        layout.root.panes().len(),
        2,
        "the mirror's tree holds {} pane(s): {}",
        layout.root.panes().len(),
        layout.root
    );
    assert!(
        matches!(&layout.root, LayoutNode::Split { axis, .. } if *axis == SplitAxis::Columns),
        "a split to the right is columns, not {}",
        layout.root
    );
    for pane in layout.root.panes() {
        assert!(mirror.pane(pane).is_some(), "the tree names {pane}, which the mirror lacks");
    }
}

/// A pane's agent state can move before its watcher is listening, and the state must survive
/// it.
///
/// herdr delivers agent state only to a subscriber that names the pane, so a watcher is
/// spawned when the structure stream says the pane exists and dials afterwards. Anything that
/// fires in between reaches nobody, and herdr has no replay - so without the read that
/// follows subscribing, the pane keeps whatever state it had and looks calm. In the app that
/// is the founding desideratum failing silently at the worst moment: right after a split,
/// with something new started in the pane.
///
/// Split and report back to back, many times, because the window is small and real. This is
/// the shape that was first filed as a flaky suite before the flakiness turned out to be the
/// symptom rather than the fault.
#[test]
fn an_agent_state_that_lands_before_its_watcher_is_recovered() {
    // Enough that the window is hit rather than hoped for. Each round is a split and a report
    // with nothing in between, which is as close as a test can get to an agent that starts
    // working the instant its pane exists.
    const ROUNDS: usize = 20;

    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));

    let (mirror, log, _subscription) = mirror_and_log(&daemon);
    until("the first bootstrap", || log.bootstraps() > 0);

    let root = {
        let mirror = mirror.lock().unwrap();
        mirror.panes().next().expect("the workspace came with a pane").id.clone()
    };

    let mut made = Vec::new();
    for _ in 0..ROUNDS {
        let answer = daemon
            .call("pane.split", &json!({ "target_pane_id": root.as_str(), "direction": "down" }));
        let Some(pane) =
            answer.get("pane").and_then(|pane| pane.get("pane_id")).and_then(Value::as_str)
        else {
            panic!("herdr split without saying which pane it made: {answer}");
        };
        let pane = pane.to_string();
        daemon.call(
            "pane.report_agent",
            &json!({
                "pane_id": pane,
                "source": "test",
                "agent": "claude",
                "state": "working",
            }),
        );
        made.push(PaneId::new(pane));
    }

    until("every new pane to reach the mirror", || {
        let mirror = mirror.lock().unwrap();
        made.iter().all(|pane| mirror.agent_state(pane).is_some())
    });

    // No further events are coming: every report has been sent and answered. So whatever the
    // mirror holds now is what a window would be showing, indefinitely.
    until("every new pane's agent state to settle", || {
        let mirror = mirror.lock().unwrap();
        made.iter().all(|pane| mirror.agent_state(pane) == Some(AgentState::Working))
    });

    let mirror = mirror.lock().unwrap();
    let calm: Vec<&PaneId> =
        made.iter().filter(|pane| mirror.agent_state(pane) != Some(AgentState::Working)).collect();
    assert!(
        calm.is_empty(),
        "{} of {ROUNDS} panes report an agent that is working and look idle here: {calm:?}.\n  \
         Impact: in the app those panes show nothing happening while an agent runs in them, \
         and nothing ever corrects it - herdr has no replay and only a reconnect \
         re-bootstraps.\n  Check the read that follows subscribing in \
         `subscription.rs::seed`.",
        calm.len()
    );
}
