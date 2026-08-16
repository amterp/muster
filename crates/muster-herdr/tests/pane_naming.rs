//! The two names a pane can have, carried from a real herdr into the mirror.
//!
//! What only a live daemon can answer. `mirror.json` pins what the mirror does with a name
//! and a title once it has them, and `backend-events.json` pins the decode - but neither
//! says herdr ever sends one, and until this file existed nothing in the repo had watched
//! it. `pane_updated` is the single live route by which a changing title reaches a sidebar,
//! and it appeared zero times across the whole corpus before the `naming` recording
//! (`observations/herdr-0.8.0.md` section 16). A feature resting on an event that never
//! fires passes every test that does not involve a daemon.
//!
//! The other half is the rename, which herdr announces to nobody: it answers with the pane
//! and emits nothing. So a client learns its own rename from the reply, and these assert on
//! what a *second* client - the subscription - ends up holding, which is the thing a window
//! actually renders.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use herdr_harness::Daemon;
use muster_core::intent::{BackendChannel, BackendIntent};
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::PaneId;
use muster_herdr::snapshot::read_snapshot;
use muster_herdr::subscription::Subscription;
use serde_json::json;

fn session() -> (Daemon, Arc<Mutex<Mirror>>, Subscription, PaneId) {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));

    let mirror = Arc::new(Mutex::new(Mirror::new()));
    let subscription = Subscription::start(
        daemon.socket_path().to_string_lossy().into_owned(),
        Arc::clone(&mirror),
        Arc::new(|_| {}),
    );
    until("the first pane to reach the mirror", || mirror.lock().unwrap().panes().count() == 1);

    let pane =
        mirror.lock().unwrap().panes().next().expect("a workspace comes with a pane").id.clone();
    (daemon, mirror, subscription, pane)
}

/// Sets the pane's title the way a harness does, through the shell rather than by injection,
/// so what herdr parses is what a real agent would have written.
fn set_title(daemon: &Daemon, pane: &PaneId, title: &str) {
    daemon.call(
        "pane.send_text",
        &json!({ "pane_id": pane.as_str(), "text": format!("printf '\\033]2;{title}\\007'\n") }),
    );
}

fn held(mirror: &Arc<Mutex<Mirror>>, pane: &PaneId) -> (Option<String>, Option<String>) {
    let mirror = mirror.lock().unwrap();
    let pane = mirror.panes().find(|held| &held.id == pane).expect("the pane left the mirror");
    (pane.name.clone(), pane.title.clone())
}

/// Takes a fresh snapshot into the mirror, which is how a name reaches a client at all.
///
/// herdr announces a rename to nobody and stamps no counter for one, so nothing on the event
/// stream is evidence about a name. What a reconnect does, done deliberately.
fn resnapshot(daemon: &Daemon, mirror: &Arc<Mutex<Mirror>>) {
    let fetched = daemon.call("session.snapshot", &json!({}));
    let (snapshot, _dropped) =
        read_snapshot(fetched.get("snapshot").expect("a snapshot with no snapshot in it"));
    mirror.lock().unwrap().bootstrap(snapshot);
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
fn a_title_the_program_sets_reaches_the_mirror_without_being_asked_for() {
    let (daemon, mirror, _subscription, pane) = session();

    set_title(&daemon, &pane, "first working build");
    until("the title to arrive on the subscription", || {
        held(&mirror, &pane).1.as_deref() == Some("first working build")
    });

    // A second one, because the first could have ridden the bootstrap snapshot rather than
    // an event - and it is the event path that keeps a sidebar current on a live session.
    set_title(&daemon, &pane, "second working build");
    until("a later title to replace it", || {
        held(&mirror, &pane).1.as_deref() == Some("second working build")
    });
}

#[test]
fn a_spinning_glyph_in_front_of_an_unchanged_title_is_not_a_change() {
    let (daemon, mirror, _subscription, pane) = session();

    set_title(&daemon, &pane, "first working build");
    until("the title to arrive", || {
        held(&mirror, &pane).1.as_deref() == Some("first working build")
    });

    // The budget case: a harness rotates this several times a second, and the roster is
    // republished per relabel. herdr compares the stripped title before announcing, so what
    // reaches Muster should be nothing at all - and the assertion is on the *stripped* text
    // arriving, because a glyph leaking through would show up as a subtitle that flickers.
    for glyph in ['✳', '✻', '·', '✽', '✢'] {
        set_title(&daemon, &pane, &format!("{glyph} first working build"));
        thread::sleep(Duration::from_millis(200));
    }
    thread::sleep(Duration::from_millis(500));

    assert_eq!(
        held(&mirror, &pane).1.as_deref(),
        Some("first working build"),
        "an activity glyph reached the mirror, so every spin would redraw the row"
    );
}

#[test]
fn a_name_and_a_title_sit_beside_each_other_and_neither_overwrites_the_other() {
    let (daemon, mirror, _subscription, pane) = session();

    set_title(&daemon, &pane, "first working build");
    until("the title to arrive", || held(&mirror, &pane).1.is_some());

    daemon.call("pane.rename", &json!({ "pane_id": pane.as_str(), "label": "🔥 payments spike" }));
    resnapshot(&daemon, &mirror);

    // The compatibility the whole feature rests on: herdr keeps the two as separate fields
    // and setting one never touches the other, so a title arriving after a rename leaves the
    // name where it is - and it arrives on the stream, without another snapshot.
    set_title(&daemon, &pane, "second working build");
    until("a later title to arrive", || {
        held(&mirror, &pane).1.as_deref() == Some("second working build")
    });

    assert_eq!(
        held(&mirror, &pane),
        (Some("🔥 payments spike".to_string()), Some("second working build".to_string())),
        "naming a pane cost it the ability to say what it is doing, or the reverse"
    );
}

#[test]
fn renaming_through_the_intent_shows_up_without_waiting_for_a_snapshot() {
    // The whole point of the feature, and the hole it was shipped with. herdr emits no event
    // for a pane rename, so a client that only listens changes the daemon and not its own
    // window - which is what the running app did, while the wire-level cases were green.
    // Going through `submit` rather than the raw call is what makes this the real path.
    let (daemon, mirror, _subscription, pane) = session();
    let client = daemon.client();

    let outcome = client
        .submit(&BackendIntent::RenamePane {
            pane: pane.clone(),
            name: Some("🔥 payments spike".into()),
        })
        .expect("a real daemon refused a rename it can do");
    let (renamed, name) = outcome.renamed.clone().expect("pane.rename did not answer with a name");
    assert_eq!(renamed, pane);
    mirror.lock().unwrap().rename(&renamed, name);

    assert_eq!(
        held(&mirror, &pane).0.as_deref(),
        Some("🔥 payments spike"),
        "the answer to a rename was not applied, so the window keeps the old name until it \
         happens to re-snapshot"
    );

    // And the way back, which is a null on the wire and no name here.
    let outcome = client
        .submit(&BackendIntent::RenamePane { pane: pane.clone(), name: None })
        .expect("a real daemon refused a rename it can do");
    let (cleared, name) = outcome.renamed.clone().expect("a clearing rename answered with nothing");
    mirror.lock().unwrap().rename(&cleared, name);

    assert_eq!(held(&mirror, &pane).0, None, "clearing a name left it named");
}

#[test]
fn a_reconnect_does_not_put_back_a_name_the_session_has_moved_past() {
    // The bug this file was extended for, and the one only a real daemon reproduces: the
    // replay is a ring buffer of past events, drained after the snapshot rather than before,
    // so a rename made before a client connected arrives twice - once correctly on the
    // snapshot, then again as the payload that preceded it.
    let (daemon, _first, _subscription, pane) = session();

    daemon.call("pane.rename", &json!({ "pane_id": pane.as_str(), "label": "🔥 payments spike" }));
    set_title(&daemon, &pane, "first working build");
    thread::sleep(Duration::from_millis(800));
    daemon.call("pane.rename", &json!({ "pane_id": pane.as_str(), "label": "🧪 flaky test hunt" }));
    thread::sleep(Duration::from_millis(400));

    // A second client, arriving now, which is what a reconnect looks like from the daemon's
    // side: snapshot first, then every event the ring still holds.
    let fresh = Arc::new(Mutex::new(Mirror::new()));
    let _following = Subscription::start(
        daemon.socket_path().to_string_lossy().into_owned(),
        Arc::clone(&fresh),
        Arc::new(|_| {}),
    );
    until("the fresh mirror to see the pane", || fresh.lock().unwrap().panes().count() == 1);
    thread::sleep(Duration::from_millis(800));

    assert_eq!(
        held(&fresh, &pane).0.as_deref(),
        Some("🧪 flaky test hunt"),
        "a replayed event put back a name the session had moved past, so the window shows one \
         thing and the daemon holds another and nothing ever corrects it"
    );
}

#[test]
fn a_name_cleared_by_somebody_else_arrives_only_on_a_fresh_snapshot() {
    let (daemon, mirror, _subscription, pane) = session();

    daemon.call("pane.rename", &json!({ "pane_id": pane.as_str(), "label": "🔥 payments spike" }));
    set_title(&daemon, &pane, "first working build");
    resnapshot(&daemon, &mirror);
    assert_eq!(held(&mirror, &pane).0.as_deref(), Some("🔥 payments spike"));

    // Null rather than empty, which is the only spelling herdr's schema accepts for a pane
    // and the only one that clears.
    daemon.call("pane.rename", &json!({ "pane_id": pane.as_str(), "label": null }));
    set_title(&daemon, &pane, "second working build");
    until("a later title to arrive", || {
        held(&mirror, &pane).1.as_deref() == Some("second working build")
    });

    // Still named, and deliberately. Nothing on the stream is evidence about a name: herdr
    // announces a rename to nobody and stamps no counter for one, so a `label` riding a
    // title change is whatever was true when that payload was built rather than news.
    //
    // Muster's own clears do not go through here at all - a rename is applied from the
    // answer herdr gives it, the way a split is. What this pins is the limit for a rename
    // somebody else made: it shows up when the connection next re-snapshots.
    assert_eq!(
        held(&mirror, &pane).0.as_deref(),
        Some("🔥 payments spike"),
        "an event wrote a name, which is what lets a reconnect put back one already moved past"
    );

    resnapshot(&daemon, &mirror);

    assert_eq!(
        held(&mirror, &pane),
        (None, Some("second working build".to_string())),
        "a fresh snapshot did not settle the name, or took the title with it"
    );
}
