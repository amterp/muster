//! What a daemon restart keeps (MIP-3 section 2): every tab's shape, each pane's name and
//! directory, and the settings an app gave. Never a process, a command, a title, or what an
//! agent said about itself.

mod support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use muster_harness::Input;
use proto::input_event::{self, Input as Event};
use proto::{pane_request, placement, session_request, tab_request};
use support::*;

fn state_file(daemon: &Daemon) -> PathBuf {
    daemon.root().join("daemon.state.json")
}

fn stop(daemon: &mut Daemon, control: &mut Control) {
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
fn until_restored(control: &mut Control, panes: usize) -> proto::Snapshot {
    until_some(&format!("{panes} panes to be restored"), || {
        let snapshot = snapshot(control);
        (snapshot.panes.len() == panes).then_some(snapshot)
    })
}

fn record<'a>(snapshot: &'a proto::Snapshot, pane: &str) -> &'a proto::Pane {
    snapshot.panes.iter().find(|record| record.pane == pane).unwrap_or_else(|| panic!("no {pane}"))
}

fn until_saved(daemon: &Daemon, needle: &str) {
    until(
        &format!("the state file to hold {needle}"),
        || written(&state_file(daemon)).contains(needle),
        (),
    );
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
    assert_eq!(after.settings.and_then(|settings| settings.palette), Some(palette));

    type_line(&mut input, "p3", "pwd");
    until_text(&mut control, "p3", &moved.display().to_string());
    type_line(&mut input, "p2", "echo p2-is-a-shell");
    until_text(&mut control, "p2", "p2-is-a-shell\n");
    assert_eq!(written(&ran), "ran\n", "the command ran once, before the restart and not after");
}

#[test]
fn a_crash_keeps_what_was_written_shortly_before_it() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_saved(&daemon, "\"p1\"");
    daemon.kill();
    daemon.restart();
    let mut control = daemon.connect();
    until_restored(&mut control, 1);
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
    assert!(snapshot(&mut control).tabs.is_empty());
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
