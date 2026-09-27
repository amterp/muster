//! Subscribing, settings, and the requests this daemon does not serve yet.

mod support;

use std::time::Duration;

use proto::session_request;
use support::*;

#[test]
fn a_subscription_starts_from_its_snapshot_and_hears_only_what_follows() {
    let daemon = daemon();
    let mut writer = daemon.connect();
    make(&mut writer, create("p1", in_new_tab("t1")));

    let mut watcher = daemon.connect();
    let asked = expect(&mut watcher, subscribe_request(), proto::Outcome::Done);
    let Some(proto::answer::Detail::Snapshot(snapshot)) = asked.answer.detail else {
        panic!("a subscription answered without a snapshot");
    };
    assert!(asked.events.is_empty(), "nothing from before the subscription is replayed");
    assert_eq!(snapshot.seq, asked.answer.seq);
    assert_eq!(snapshot.instance, watcher.welcome().instance);
    assert_eq!(snapshot.panes.len(), 1);

    make(&mut writer, create("p2", beside("p1", proto::Side::Right)));
    let heard = [watcher.next_event(), watcher.next_event()];
    assert_eq!(names(&heard), ["pane_opened:p2", "tab_changed:t1"]);
    assert_eq!(heard[0].seq, snapshot.seq + 1);
    assert_eq!(heard[1].seq, snapshot.seq + 2);
}

/// A subscription made while another connection is changing things still starts from its
/// snapshot: no event reaches it before the answer, and the first one after is the next in
/// sequence. An event queued between taking the snapshot and queueing its answer would arrive
/// ahead of a snapshot older than it.
#[test]
fn a_subscription_made_amid_changes_hears_nothing_before_its_snapshot() {
    let daemon = daemon();
    amid_changes(&daemon, || {
        let mut watcher = daemon.connect();
        let asked = expect(&mut watcher, subscribe_request(), proto::Outcome::Done);
        let Some(proto::answer::Detail::Snapshot(snapshot)) = asked.answer.detail else {
            panic!("a subscription answered without a snapshot");
        };
        assert_eq!(names(&asked.events), Vec::<String>::new(), "events before the snapshot");
        assert_eq!(watcher.next_event().seq, snapshot.seq + 1, "the event after the snapshot");
    });
}

/// A subscriber that asks for a snapshot again, to resynchronize, hears every event it covers
/// before it and none that it does not: a later event ahead of it would be rolled back by it.
#[test]
fn a_snapshot_asked_amid_changes_arrives_after_exactly_the_events_it_covers() {
    let daemon = daemon();
    let mut watcher = daemon.connect();
    expect(&mut watcher, subscribe_request(), proto::Outcome::Done);
    amid_changes(&daemon, || {
        let asked = expect(&mut watcher, snapshot_request(), proto::Outcome::Done);
        let Some(proto::answer::Detail::Snapshot(snapshot)) = asked.answer.detail else {
            panic!("a snapshot answered without one");
        };
        let later: Vec<u64> =
            asked.events.iter().map(|event| event.seq).filter(|seq| *seq > snapshot.seq).collect();
        assert_eq!(later, Vec::<u64>::new(), "events newer than the snapshot, ahead of it");
    });
}

/// Runs `check` 200 times while another connection renames a pane as fast as it can.
fn amid_changes(daemon: &Daemon, mut check: impl FnMut()) {
    let mut writer = daemon.connect();
    make(&mut writer, create("p1", in_new_tab("t1")));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let changing = {
        let (stop, socket) = (std::sync::Arc::clone(&stop), daemon.socket_path().to_path_buf());
        std::thread::spawn(move || {
            let mut writer = Control::connect(&socket);
            for turn in 0.. {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                let label = Some(format!("label {turn}"));
                let rename = proto::pane_request::Rename { pane: "p1".to_string(), label };
                writer.ask(pane(proto::pane_request::Request::Rename(rename)));
            }
        })
    };
    for _ in 0..200 {
        check();
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    changing.join().unwrap();
}

#[test]
fn a_connection_that_did_not_subscribe_hears_no_events() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let asked = make(&mut control, create("p1", in_new_tab("t1")));
    assert!(asked.events.is_empty());
    // What proves a negative here is that the answer arrived: the daemon writes a request's
    // events before its answer, so none were coming.
    assert!(control.next_message(Duration::from_millis(50)).is_none());
}

#[test]
fn settings_are_announced_when_they_change_and_not_otherwise() {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    let set_scrollback =
        |bytes| session(session_request::Request::SetScrollback(proto::SetScrollback { bytes }));
    let asked = expect(&mut control, set_scrollback(Some(1_000_000)), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["settings_changed"]);
    expect(&mut control, set_scrollback(Some(1_000_000)), proto::Outcome::AlreadySo);

    let palette = proto::Palette {
        entries: vec![0x00_00_00, 0xff_00_00],
        foreground: 0xff_ff_ff,
        background: 0x00_00_00,
        cursor: None,
        scheme: proto::ColorScheme::Dark.into(),
    };
    let set_palette = |palette: proto::Palette| {
        session(session_request::Request::SetPalette(proto::SetPalette { palette: Some(palette) }))
    };
    expect(&mut control, set_palette(palette.clone()), proto::Outcome::Done);
    expect(&mut control, set_palette(palette), proto::Outcome::AlreadySo);
    let too_many = proto::Palette { entries: vec![0; 257], ..proto::Palette::default() };
    expect(&mut control, set_palette(too_many), proto::Outcome::Refused);

    let set_clipboard_write = |allowed| {
        session(session_request::Request::SetClipboardWrite(proto::SetClipboardWrite { allowed }))
    };
    expect(&mut control, set_clipboard_write(true), proto::Outcome::AlreadySo);
    expect(&mut control, set_clipboard_write(false), proto::Outcome::Done);

    let block = proto::Cursor { style: proto::CursorStyle::Block.into(), blink: Some(false) };
    let set_cursor = |cursor: Option<proto::Cursor>| {
        session(session_request::Request::SetCursor(proto::SetCursor { cursor }))
    };
    expect(&mut control, set_cursor(Some(block)), proto::Outcome::Done);
    expect(&mut control, set_cursor(Some(block)), proto::Outcome::AlreadySo);
    expect(&mut control, set_cursor(None), proto::Outcome::Refused);

    let settings = snapshot(&mut control).settings.unwrap();
    assert_eq!(settings.scrollback_bytes, Some(1_000_000));
    assert_eq!(settings.palette.unwrap().entries.len(), 2);
    assert_eq!(settings.clipboard_write, Some(false));
    assert_eq!(settings.cursor, Some(block));

    let manifests = proto::SendManifests {
        engine: 1,
        manifests: vec![proto::Manifest { agent: "claude".to_string(), toml: String::new() }],
    };
    let send = session(session_request::Request::SendManifests(manifests));
    expect(&mut control, send.clone(), proto::Outcome::Done);
    expect(&mut control, send, proto::Outcome::AlreadySo);
}

#[test]
fn a_pane_runs_the_shell_it_was_set_to_run() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let marker = daemon.root().join("which");
    let script = daemon.root().join("shell");
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho \"$@\" > {}\nexec /bin/sh\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let shell = proto::Shell {
        command: Some(script.display().to_string()),
        mode: proto::ShellMode::NonLogin.into(),
        ..proto::Shell::default()
    };
    let set = session(session_request::Request::SetShell(proto::SetShell { shell: Some(shell) }));
    expect(&mut control, set.clone(), proto::Outcome::Done);
    expect(&mut control, set, proto::Outcome::AlreadySo);
    make(&mut control, create("p1", in_new_tab("t1")));
    assert_eq!(
        written(&marker),
        "-i\n",
        "a non-login shell is started interactive and nothing else"
    );
}

#[test]
fn a_request_this_daemon_does_not_know_is_refused_rather_than_misread() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let asked = expect(
        &mut control,
        proto::request::Service::Pane(proto::PaneRequest { request: None }),
        proto::Outcome::Refused,
    );
    assert!(asked.answer.reason.contains("does not know"), "{}", asked.answer.reason);
}
