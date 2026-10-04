//! What a daemon restart keeps (MIP-3 section 2): every tab's shape, each pane's name and
//! directory, and the settings an app gave. Never a process, a command, a title, or what an
//! agent said about itself.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::support::*;
use muster_harness::Input;
use proto::input_event::{self, Input as Event};
use proto::{pane_request, placement, session_request, tab_request};

fn state_file(daemon: &Daemon) -> PathBuf {
    daemon.root().join("daemon.state.json")
}

pub(crate) fn stop(daemon: &mut Daemon, control: &mut Control) {
    expect(
        control,
        session(session_request::Request::Stop(session_request::Stop {})),
        proto::Outcome::Done,
    );
    daemon.wait_for_exit();
}

fn type_line(input: &mut Input, pane: &str, text: &str) {
    input.send(pane, Event::Send(input_event::Send { text: text.to_string(), enter: true }));
}

fn directory(daemon: &Daemon, name: &str) -> PathBuf {
    let path = daemon.root().join(name);
    std::fs::create_dir_all(&path).unwrap();
    canonical(&path)
}

/// Waits until a restarted daemon has brought back `panes` panes.
pub(crate) fn until_restored(control: &mut Control, panes: usize) -> proto::Snapshot {
    until_some(&format!("{panes} panes to be restored"), || {
        let snapshot = snapshot(control);
        (snapshot.panes.len() == panes).then_some(snapshot)
    })
}

pub(crate) fn record<'a>(snapshot: &'a proto::Snapshot, pane: &str) -> &'a proto::Pane {
    snapshot.panes.iter().find(|record| record.pane == pane).unwrap_or_else(|| panic!("no {pane}"))
}

pub(crate) fn until_saved(daemon: &Daemon, needle: &str) {
    until(
        &format!("the state file to hold {needle}"),
        || written(&state_file(daemon)).contains(needle),
        (),
    );
}

/// Every setting but the palette, each away from its default.
fn set_the_other_settings(control: &mut Control) {
    let shell = proto::Shell {
        command: None,
        mode: proto::ShellMode::NonLogin.into(),
        ..proto::Shell::default()
    };
    let set = proto::SetShell { shell: Some(shell) };
    expect(control, session(session_request::Request::SetShell(set)), proto::Outcome::Done);
    let set = proto::SetScrollback { bytes: Some(2_000_000) };
    expect(control, session(session_request::Request::SetScrollback(set)), proto::Outcome::Done);
    let set = proto::SetClipboardWrite { allowed: false };
    let request = session_request::Request::SetClipboardWrite(set);
    expect(control, session(request), proto::Outcome::Done);
    let cursor =
        proto::Cursor { style: proto::CursorStyle::Unspecified.into(), blink: Some(false) };
    let set = proto::SetCursor { cursor: Some(cursor) };
    expect(control, session(session_request::Request::SetCursor(set)), proto::Outcome::Done);
    let set = proto::SetScrollMultiplier { multiplier: 0.5 };
    let request = session_request::Request::SetScrollMultiplier(set);
    expect(control, session(request), proto::Outcome::Done);
}

#[test]
fn a_restart_brings_back_every_tab_its_names_and_directories_and_nothing_else() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    let (first, second, moved) =
        (directory(&daemon, "first"), directory(&daemon, "second"), directory(&daemon, "moved"));
    let ran = daemon.root().join("ran");

    make(
        &mut control,
        pane_request::Create {
            cwd: Some(first.display().to_string()),
            ..create("p1", in_new_tab("t1"))
        },
    );
    let beside = proto::Placement {
        r#where: Some(placement::Where::Beside(placement::Beside {
            pane: "p1".to_string(),
            side: proto::Side::Right.into(),
            ratio: Some(0.3),
        })),
    };
    make(
        &mut control,
        pane_request::Create {
            command: Some(format!("echo ran >> '{}'", ran.display())),
            ..create("p2", beside)
        },
    );
    make(
        &mut control,
        pane_request::Create {
            cwd: Some(second.display().to_string()),
            ..create("p3", in_new_tab("t2"))
        },
    );
    let zoom = pane_request::Zoom { pane: "p2".to_string(), zoomed: true };
    expect(&mut control, pane(pane_request::Request::Zoom(zoom)), proto::Outcome::Done);
    let label = proto::Label { text: Some("work".to_string()), generation: 2 };
    let rename = tab_request::Rename { tab: "t1".to_string(), label: Some(label) };
    expect(&mut control, tab(tab_request::Request::Rename(rename)), proto::Outcome::Done);
    let rename = pane_request::Rename { pane: "p1".to_string(), label: Some("🤖 A".to_string()) };
    expect(&mut control, pane(pane_request::Request::Rename(rename)), proto::Outcome::Done);
    let palette = proto::Palette {
        entries: vec![0x10_20_30; 16],
        foreground: 0xee_ee_ee,
        ..Default::default()
    };
    let set = proto::SetPalette { palette: Some(palette.clone()) };
    expect(&mut control, session(session_request::Request::SetPalette(set)), proto::Outcome::Done);
    set_the_other_settings(&mut control);
    // A size its bridge gave it.
    let mut stream = attached(&daemon, "p3", false);
    stream.resize(proto::Grid { cols: 100, rows: 30, width_px: 1000, height_px: 600 });
    until_some("p3 to take its bridge's size", || {
        type_line(&mut input, "p3", "stty size");
        read_text(&mut control, "p3", 0, 0).text.contains("30 100").then_some(())
    });
    drop(stream);
    let report = pane_request::Report {
        pane: "p1".to_string(),
        model: Some("Opus".to_string()),
        ..Default::default()
    };
    expect(&mut control, pane(pane_request::Request::Report(report)), proto::Outcome::Done);
    type_line(&mut input, "p3", &format!("cd '{}'", moved.display()));
    until_some("p3's directory to follow its cd", || {
        (record(&snapshot(&mut control), "p3").cwd == moved.display().to_string()).then_some(())
    });
    until_file(&ran, "p2's command to run");
    let before = snapshot(&mut control);

    stop(&mut daemon, &mut control);
    daemon.restart();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    let after = until_restored(&mut control, 3);

    let by_name = |snapshot: &proto::Snapshot| {
        let mut tabs = snapshot.tabs.clone();
        tabs.sort_by(|one, other| one.tab.cmp(&other.tab));
        tabs
    };
    assert_eq!(by_name(&after), by_name(&before), "tabs, trees, ratios, zoom and labels");
    for name in ["p1", "p2", "p3"] {
        let (was, is) = (record(&before, name), record(&after, name));
        assert_eq!((&is.label, &is.cwd), (&was.label, &was.cwd), "{name}'s label and directory");
        assert_eq!(is.command, None, "{name} is a shell, whatever it was started with");
        assert_eq!(is.facts, None, "what an agent said about itself is not kept");
        assert_eq!((is.agent.as_deref(), is.agent_state()), (None, proto::AgentState::Unknown));
    }
    assert_eq!(after.settings, before.settings, "every setting");
    assert_eq!(after.settings.and_then(|settings| settings.palette), Some(palette));

    type_line(&mut input, "p3", "pwd");
    until_text(&mut control, "p3", &moved.display().to_string());
    type_line(&mut input, "p2", "echo p2-is-a-shell");
    until_text(&mut control, "p2", "p2-is-a-shell\n");
    assert_eq!(written(&ran), "ran\n", "the command ran once, before the restart and not after");
    type_line(&mut input, "p3", "clear; stty size");
    until_text(&mut control, "p3", "30 100\n");
    // The kept cursor reaches a pane made after the restart, as the app's [cursor] would.
    make(&mut control, create("p4", in_new_tab("t3")));
    type_line(&mut input, "p4", "echo \"features=$GHOSTTY_SHELL_FEATURES\"");
    until_text(&mut control, "p4", "features=cursor:steady,path,ssh-env,ssh-terminfo,title\n");
}

/// What a program printed and the title it set belong to its terminal, which ends with it.
#[test]
fn a_restart_brings_back_no_scrollback_and_no_title() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    make(&mut control, create("p1", in_new_tab("t1")));
    type_line(&mut input, "p1", "printf '\\033]2;before the restart\\007'; echo printed-before");
    until_text(&mut control, "p1", "printed-before\n");
    until_some("p1's title", || {
        (record(&snapshot(&mut control), "p1").title == "before the restart").then_some(())
    });
    until_saved(&daemon, "p1");

    stop(&mut daemon, &mut control);
    daemon.restart();
    let mut control = daemon.connect();
    let after = until_restored(&mut control, 1);
    assert_ne!(record(&after, "p1").title, "before the restart", "no title");
    let text = read_text(&mut control, "p1", 0, 0).text;
    assert!(!text.contains("printed-before"), "no scrollback: {text:?}");
}

#[test]
fn a_crash_keeps_what_was_written_shortly_before_it() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    assert!(!snapshot(&mut control).restored_from_file, "p1 was made by this run");
    until_saved(&daemon, "\"p1\"");
    daemon.kill();
    daemon.restart();
    let mut control = daemon.connect();
    let after = until_restored(&mut control, 1);
    assert!(after.restored_from_file, "a window has to learn p1's processes are new");
}

#[test]
fn a_daemon_told_to_stop_by_a_signal_writes_what_it_held_at_once() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    // SAFETY: kill signals the test's own daemon process.
    unsafe { libc::kill(daemon.pid().cast_signed(), libc::SIGTERM) };
    daemon.wait_for_exit();
    daemon.restart();
    let mut control = daemon.connect();
    until_restored(&mut control, 1);
}

#[test]
fn a_file_from_a_newer_daemon_is_refused_and_left_as_it_was() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    stop(&mut daemon, &mut control);
    let newer = b"{\"version\": 999, \"tabs\": \"in a shape this daemon has never seen\"}";
    std::fs::write(state_file(&daemon), newer).unwrap();

    daemon.restart();
    let mut control = daemon.connect();
    assert!(snapshot(&mut control).tabs.is_empty());
    make(&mut control, create("p1", in_new_tab("t1")));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(bytes_in(&state_file(&daemon)), newer, "the newer daemon's file was replaced");
    assert!(written(&daemon.root().join("daemon.log")).contains("daemon.state.newer"));
}

#[test]
fn a_damaged_file_is_moved_aside_and_the_daemon_starts_empty() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    stop(&mut daemon, &mut control);
    let damaged = b"{\"version\": 1, \"tabs\": [{\"tab\": \"t1\", \"ro";
    std::fs::write(state_file(&daemon), damaged).unwrap();

    daemon.restart();
    let mut control = daemon.connect();
    let started = snapshot(&mut control);
    assert!(started.tabs.is_empty());
    assert!(!started.restored_from_file, "nothing came back from a file it could not read");
    let aside = moved_aside(daemon.root()).expect("the damaged file was kept");
    assert_eq!(bytes_in(&aside), damaged);
    assert!(written(&daemon.root().join("daemon.log")).contains("daemon.state.corrupt"));
    make(&mut control, create("p1", in_new_tab("t1")));
    until_saved(&daemon, "\"p1\"");
}

fn moved_aside(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.to_string_lossy().contains("daemon.state.json.corrupt-"))
}

/// The write happens with the session unlocked. A FIFO where the temporary file goes stands in
/// for a disk that never finishes a write: opening it waits for a reader that never comes.
#[test]
fn a_write_that_never_finishes_holds_up_no_request() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let fifo =
        std::ffi::CString::new(daemon.root().join("daemon.state.json.tmp").to_str().unwrap())
            .unwrap();
    // SAFETY: mkfifo reads a NUL-terminated path the test owns.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    make(&mut control, create("p1", in_new_tab("t1")));
    std::thread::sleep(Duration::from_millis(1500));

    let started = Instant::now();
    make(&mut control, create("p2", in_new_tab("t2")));
    snapshot(&mut control);
    let took = started.elapsed();
    assert!(took < Duration::from_secs(1), "a create and a snapshot took {took:?}");
}

/// A state file of `tabs` tabs of one pane each, `t<n>` holding `p<n>`, every pane in `cwd`,
/// with `shell` as the configured shell if one is given.
fn saved(tabs: usize, cwd: &Path, shell: Option<&str>) -> String {
    let shell = shell.map_or("null".to_string(), |shell| format!("{shell:?}"));
    let tab =
        |n| format!(r#"{{"tab":"t{n}","label":{{"generation":0}},"root":{{"pane":"p{n}"}}}}"#);
    let pane = |n| {
        format!(
            r#"{{"pane":"p{n}","cwd":{:?},"grid":{{"cols":80,"rows":24,"width_px":0,"height_px":0}}}}"#,
            cwd.display().to_string()
        )
    };
    format!(
        r#"{{"version":1,"daemon":"test","settings":{{"shell":{{"command":{shell},"mode":0}}}},"tabs":[{}],"panes":[{}]}}"#,
        (0..tabs).map(tab).collect::<Vec<_>>().join(","),
        (0..tabs).map(pane).collect::<Vec<_>>().join(","),
    )
}

/// Stops the daemon, puts `file` where its state is kept, and starts it again on it.
fn restarted_from(daemon: &mut Daemon, file: &str) -> Control {
    let mut control = daemon.connect();
    stop(daemon, &mut control);
    std::fs::write(state_file(daemon), file).unwrap();
    daemon.restart();
    daemon.connect()
}

/// The copy of the state file a restore that lost something kept aside.
fn kept_aside(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root).unwrap().flatten().map(|entry| entry.path()).find(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("daemon.state.json.unrestored-"))
    })
}

fn restored(events: &[proto::Event]) -> Option<&proto::Restored> {
    events.iter().find_map(|event| match &event.event {
        Some(proto::event::Event::Restored(restored)) => Some(restored),
        _ => None,
    })
}

/// Only the shell went bad, so the pane keeps its directory.
#[test]
fn a_shell_gone_since_the_last_run_comes_back_as_the_default_shell() {
    let mut daemon = daemon();
    let work = directory(&daemon, "work");
    let mut control = restarted_from(&mut daemon, &saved(1, &work, Some("/nonexistent/shell")));
    let snapshot = until_restored(&mut control, 1);
    assert_eq!(record(&snapshot, "p0").cwd, work.display().to_string());
    until_text(&mut control, "p0", "");
    assert!(written(&daemon.root().join("daemon.log")).contains("daemon.state.fallback"));
    assert_eq!(kept_aside(daemon.root()), None, "nothing was lost");
}

/// With no shell that will start, a tab cannot come back; the next write leaves it out of the
/// file, so the file as it was is kept aside first.
#[test]
fn a_tab_no_shell_will_start_for_is_kept_in_a_copy_of_the_file() {
    let mut daemon = daemon_with(&[("SHELL", "/nonexistent/default")]);
    let work = directory(&daemon, "work");
    let file = saved(1, &work, Some("/nonexistent/shell"));
    let mut control = restarted_from(&mut daemon, &file);
    let kept = until_some("a copy of the file kept aside", || kept_aside(daemon.root()));
    assert_eq!(std::fs::read_to_string(kept).unwrap(), file);
    let snapshot = until_some("the restore to end", || {
        Some(snapshot(&mut control)).filter(|snapshot| !snapshot.restoring)
    });
    assert!(snapshot.tabs.is_empty());
}

/// A client that makes a pane under a name a saved pane has, while the daemon is restoring,
/// keeps it; the saved pane is lost, and the file that held it is kept aside. Until the restore
/// has ended, the snapshot says it is under way.
#[test]
fn a_name_a_client_took_while_restoring_is_kept_in_a_copy_of_the_file() {
    // Twenty shells to start against one connection's worth of requests: about 70 ms here, and
    // ten seconds under emulation, where sixty outlasted the suite's patience.
    const TABS: usize = 20;
    let mut daemon = daemon();
    let work = directory(&daemon, "work");
    let file = saved(TABS, &work, None);
    let mut control = restarted_from(&mut daemon, &file);
    let subscribed = expect(&mut control, subscribe_request(), proto::Outcome::Done);
    let Some(proto::answer::Detail::Snapshot(first)) = subscribed.answer.detail else {
        panic!("a subscribe answers with a snapshot")
    };
    assert!(first.restoring, "{TABS} tabs take longer than one answer to bring back");
    let taken = format!("p{}", TABS - 1);
    make(&mut control, create(&taken, in_new_tab("tx")));

    let events =
        events_until(&mut control, "the restore to end", |events| restored(events).is_some());
    let restored = restored(&events).unwrap();
    assert_eq!(restored.lost_tabs, [format!("t{}", TABS - 1)]);
    assert_eq!(restored.lost_panes, [taken.as_str()]);
    assert!(!restored.saving_stopped);
    let kept = kept_aside(daemon.root()).expect("a copy kept aside");
    assert_eq!(std::fs::read_to_string(kept).unwrap(), file);
    let after = snapshot(&mut control);
    assert!(!after.restoring);
    let tab = after.tabs.iter().find(|tab| tab.tab == "tx").expect("the client's tab");
    assert_eq!(shape(tab.root.as_ref().unwrap()), taken, "the client's pane stays");
}

/// A daemon stopped while it is still bringing its tabs back holds less than the file, so it
/// writes nothing over it.
#[test]
fn a_stop_while_restoring_leaves_the_file_as_it_was() {
    const TABS: usize = 60;
    let mut daemon = daemon();
    let work = directory(&daemon, "work");
    let file = saved(TABS, &work, None);
    let mut control = restarted_from(&mut daemon, &file);
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    events_until(&mut control, "a first tab back", |events| {
        names(events).iter().any(|name| name.starts_with("tab_opened:"))
    });
    stop(&mut daemon, &mut control);
    assert_eq!(std::fs::read_to_string(state_file(&daemon)).unwrap(), file);
    assert!(written(&daemon.root().join("daemon.log")).contains("daemon.state.not_saved"));
}
