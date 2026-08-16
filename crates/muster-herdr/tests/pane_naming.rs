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
    // herdr announces a rename to nobody, so nothing arrives until the next title change
    // carries the name along. That is the finding rather than a flaw in the test: this is
    // what a second client actually experiences.
    set_title(&daemon, &pane, "second working build");
    until("the name to reach the mirror", || held(&mirror, &pane).0.is_some());

    assert_eq!(
        held(&mirror, &pane),
        (Some("🔥 payments spike".to_string()), Some("second working build".to_string())),
        "naming a pane cost it the ability to say what it is doing, or the reverse"
    );
}

#[test]
fn a_name_cleared_by_somebody_else_arrives_only_on_a_fresh_snapshot() {
    let (daemon, mirror, _subscription, pane) = session();

    daemon.call("pane.rename", &json!({ "pane_id": pane.as_str(), "label": "🔥 payments spike" }));
    set_title(&daemon, &pane, "first working build");
    until("the name and the title to arrive", || {
        let (name, title) = held(&mirror, &pane);
        name.is_some() && title.is_some()
    });

    // Null rather than empty, which is the only spelling herdr's schema accepts for a pane
    // and the only one that clears.
    daemon.call("pane.rename", &json!({ "pane_id": pane.as_str(), "label": null }));
    set_title(&daemon, &pane, "second working build");
    until("a later title to arrive", || {
        held(&mirror, &pane).1.as_deref() == Some("second working build")
    });

    // Still named, and deliberately. Two things have to be true at once for a clear to
    // arrive on the stream, and neither is: herdr announces a rename to nobody, and an
    // event may not clear a field it omits, because its replay omits both fields on every
    // reconnect (`observations/herdr-0.8.0.md` section 16).
    //
    // Muster's own clears do not go through here at all - a rename is applied from the
    // answer herdr gives it, the way a split is. What this pins is the limit for a rename
    // somebody else made: it shows up when the connection next re-snapshots.
    assert_eq!(
        held(&mirror, &pane).0.as_deref(),
        Some("🔥 payments spike"),
        "an event cleared a name, which means a reconnect can wipe every name in the window"
    );

    let fetched = daemon.call("session.snapshot", &json!({}));
    let (snapshot, _dropped) =
        read_snapshot(fetched.get("snapshot").expect("a snapshot with no snapshot in it"));
    mirror.lock().unwrap().bootstrap(snapshot);

    assert_eq!(
        held(&mirror, &pane),
        (None, Some("second working build".to_string())),
        "a fresh snapshot did not settle the name, or took the title with it"
    );
}
