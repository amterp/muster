//! Replacing a running daemon (MIP-3 section 10): every pane goes to the new daemon with its
//! process, its screen and its place, and a handoff that fails anywhere leaves the old daemon
//! serving as it was.

mod support;

use muster_harness::Input;
use proto::input_event::{self, Input as Event};
use proto::{pane_request, session_request, tab_request};
use support::*;

fn type_line(input: &mut Input, pane: &str, text: &str) {
    input.send(pane, Event::Send(input_event::Send { text: text.to_string(), enter: true }));
}

/// Two panes side by side in a labeled tab, the second zoomed and labeled, each shell having
/// said its pid.
fn two_panes(daemon: &Daemon) -> (Control, Input) {
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", proto::Side::Right)));
    let rename = tab_request::Rename {
        tab: "t1".to_string(),
        label: Some(proto::Label { text: Some("work".to_string()), generation: 3 }),
    };
    expect(&mut control, tab(tab_request::Request::Rename(rename)), proto::Outcome::Done);
    let label = pane_request::Rename { pane: "p2".to_string(), label: Some("B".to_string()) };
    expect(&mut control, pane(pane_request::Request::Rename(label)), proto::Outcome::Done);
    let zoom = pane_request::Zoom { pane: "p2".to_string(), zoomed: true };
    expect(&mut control, pane(pane_request::Request::Zoom(zoom)), proto::Outcome::Done);
    let mut input = Input::connect(daemon.socket_path());
    for name in ["p1", "p2"] {
        type_line(&mut input, name, "echo pid=$$.");
        until_said(&mut control, name, "pid");
    }
    (control, input)
}

/// What a pane's shell said to `echo <what>=$$.`, once it has said it: its pid.
fn until_said(control: &mut Control, name: &str, what: &str) -> String {
    until_some(&format!("{name}'s shell to say its {what}"), || {
        said(&read_text(control, name, 0, 0).text, what)
    })
}

/// The pid in the last `<what>=<pid>.` a pane shows. The command typed to print it shows `$$`
/// there instead, and a prompt may come before it on its line, when the typing arrived first.
fn said(text: &str, what: &str) -> Option<String> {
    text.lines().rev().find_map(|line| {
        let pid = line.rsplit_once(&format!("{what}="))?.1.strip_suffix('.')?;
        (!pid.is_empty() && pid.chars().all(|c| c.is_ascii_digit())).then(|| pid.to_string())
    })
}

/// Each pane's shell, asked again on whichever daemon serves now, is the one that said `pids`.
fn the_same_shells_answer(daemon: &Daemon, pids: &[String], context: &str) {
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    for (name, pid) in ["p1", "p2"].iter().zip(pids) {
        type_line(&mut input, name, "echo again=$$.");
        assert_eq!(&until_said(&mut control, name, "again"), pid, "{context}: {name}'s shell");
    }
}

fn pids_of_both(control: &mut Control) -> Vec<String> {
    ["p1", "p2"].iter().map(|name| until_said(control, name, "pid")).collect()
}

fn stop_signal(pid: u32) {
    // SAFETY: kill signals one process this test started.
    assert_eq!(unsafe { libc::kill(pid.cast_signed(), libc::SIGTERM) }, 0);
}

fn replaced(daemon: &mut Daemon) {
    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
}

#[test]
fn every_pane_survives_a_handoff_with_its_process_screen_and_place() {
    let mut daemon = daemon();
    let (mut control, _input) = two_panes(&daemon);
    let before = snapshot(&mut control);
    let texts: Vec<String> =
        ["p1", "p2"].iter().map(|name| read_text(&mut control, name, 0, 0).text).collect();
    let old = (daemon.pid(), control.welcome().instance);

    replaced(&mut daemon);

    let mut control = daemon.connect();
    assert_ne!((daemon.pid(), control.welcome().instance), old, "a new daemon serves");
    let after = snapshot(&mut control);
    assert_eq!(after.tabs, before.tabs, "every tab's shape, label and zoom");
    assert_eq!(after.panes, before.panes, "every pane's record");
    for (name, text) in ["p1", "p2"].iter().zip(&texts) {
        assert_eq!(&read_text(&mut control, name, 0, 0).text, text, "{name}'s screen");
    }
    let mut input = Input::connect(daemon.socket_path());
    for (name, text) in ["p1", "p2"].iter().zip(&texts) {
        type_line(&mut input, name, "echo again=$$.");
        let again = until_said(&mut control, name, "again");
        assert_eq!(Some(again), said(text, "pid"), "{name}'s shell");
    }
}

/// Output the program writes while its pane is handed over waits in the PTY for the new daemon:
/// the pane's text holds every line exactly once, across the handoff.
#[test]
fn output_written_during_a_handoff_is_neither_lost_nor_doubled() {
    let mut daemon = daemon();
    let (mut control, mut input) = two_panes(&daemon);
    // Room for every line the shell prints, as fast as it can, from before the handoff until
    // just after it.
    let scrollback = proto::SetScrollback { bytes: Some(64 << 20) };
    expect(
        &mut control,
        session(session_request::Request::SetScrollback(scrollback)),
        proto::Outcome::Done,
    );
    let stop = daemon.root().join("stop");
    // The marker quoted, so the line typed, which wraps, never shows it.
    let run = format!(
        "i=0; while [ ! -e {} ]; do echo n$i; i=$((i+1)); done; echo f'i'n",
        stop.display()
    );
    type_line(&mut input, "p1", &run);
    until_text(&mut control, "p1", "n1000\n");

    replaced(&mut daemon);
    std::fs::write(&stop, "").unwrap();

    let mut control = daemon.connect();
    let text = until_text(&mut control, "p1", "\nfin\n");
    let numbers: Vec<u32> =
        text.lines().filter_map(|line| line.strip_prefix('n')?.parse().ok()).collect();
    if let Some(at) = numbers.iter().zip(0..).position(|(number, expected)| *number != expected) {
        let around = &numbers[at.saturating_sub(3)..(at + 3).min(numbers.len())];
        panic!("a line lost or doubled across the handoff, at line {at}: {around:?}");
    }
}

/// A bridge is told the pane went to another daemon, and attaching there draws the same screen.
#[test]
fn a_bridge_is_detached_as_replaced_and_attaches_again_to_the_same_screen() {
    let mut daemon = daemon();
    let (_control, _input) = two_panes(&daemon);
    let mut stream = attached(&daemon, "p1", false);
    let mut surface = Surface::new(80, 24);
    surface.follow(&mut stream, "the replay", true, |surface| surface.replays > 0);
    let drawn = surface.screen();

    replaced(&mut daemon);

    surface.follow(&mut stream, "the detach", true, |surface| surface.detached.is_some());
    assert_eq!(surface.detached, Some(proto::DetachReason::Replaced));
    let mut again = attached(&daemon, "p1", false);
    let mut redrawn = Surface::new(80, 24);
    redrawn.follow(&mut again, "the new daemon's replay", true, |surface| surface.replays > 0);
    assert_eq!(redrawn.screen(), drawn);
}

/// A subscriber hears that the daemon was replaced, then its connection ends: it connects again
/// and starts from a snapshot of the new daemon.
#[test]
fn a_subscriber_hears_replaced_and_then_its_connection_ends() {
    let mut daemon = daemon();
    let (_control, _input) = two_panes(&daemon);
    let mut watcher = daemon.connect();
    expect(&mut watcher, subscribe_request(), proto::Outcome::Done);

    replaced(&mut daemon);

    // Whatever the panes published before the handoff may come first.
    let events = events_until(&mut watcher, "replaced", |events| {
        events.last().is_some_and(|event| named(event).starts_with("replaced:"))
    });
    let Some(proto::event::Event::Replaced(replaced)) = &events.last().unwrap().event else {
        unreachable!()
    };
    assert_eq!(replaced.daemon_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(replaced.pid, daemon.pid());
    let waiting = std::time::Instant::now();
    assert_eq!(watcher.next_message(muster_harness::PATIENCE), None, "nothing after it");
    assert!(waiting.elapsed() < muster_harness::PATIENCE, "the connection ended");
}

/// The new daemon learns a pane's process ended from its terminal closing, and whoever adopted
/// the process when the old daemon exited reaps it.
#[test]
fn a_pane_whose_shell_exits_after_a_handoff_closes_and_its_process_is_reaped() {
    let mut daemon = daemon();
    let (mut control, _input) = two_panes(&daemon);
    let pid = until_said(&mut control, "p1", "pid");

    replaced(&mut daemon);

    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    let mut input = Input::connect(daemon.socket_path());
    type_line(&mut input, "p1", "exit");
    events_until(&mut control, "p1 to close", |events| {
        names(events).iter().any(|name| name.starts_with("pane_closed:p1"))
    });
    until_some("the shell to be reaped", || process_state(&pid).is_empty().then_some(()));
    assert_eq!(tab_shape(&mut control, "t1"), "p2");
}

/// The log's file holds the old daemon's records up to its commit, then the new one's.
#[test]
fn the_log_holds_the_old_daemon_up_to_its_commit_and_then_the_new_one() {
    let mut daemon = daemon();
    let (_control, _input) = two_panes(&daemon);

    replaced(&mut daemon);

    let log = until_some("the new daemon's records", || {
        let log = std::fs::read_to_string(daemon.root().join("daemon.log")).ok()?;
        log.contains("daemon.handoff.serving").then_some(log)
    });
    let committed = log.find("daemon.handoff.committed").expect("the old daemon's commit");
    assert!(committed < log.find("daemon.handoff.serving").unwrap());
}

/// The new daemon writes the state file from when it serves.
#[test]
fn what_changes_after_a_handoff_is_saved_by_the_new_daemon() {
    let mut daemon = daemon();
    let (_control, _input) = two_panes(&daemon);

    replaced(&mut daemon);

    let mut control = daemon.connect();
    let rename = tab_request::Rename {
        tab: "t1".to_string(),
        label: Some(proto::Label { text: Some("renamed".to_string()), generation: 4 }),
    };
    expect(&mut control, tab(tab_request::Request::Rename(rename)), proto::Outcome::Done);
    until_some("the rename to be saved", || {
        let saved = std::fs::read_to_string(daemon.root().join("daemon.state.json")).ok()?;
        saved.contains("renamed").then_some(())
    });
}

/// After a refused handoff the old daemon serves as before: the same instance, its panes
/// echoing, and a new pane made.
fn still_serving(daemon: &Daemon, instance: u64) {
    let mut control = daemon.connect();
    assert_eq!(control.welcome().instance, instance, "the same daemon");
    let mut input = Input::connect(daemon.socket_path());
    type_line(&mut input, "p1", "echo still-here");
    until_text(&mut control, "p1", "still-here\n");
    make(&mut control, create("p3", in_new_tab("t2")));
    until_text(&mut control, "p3", "$");
}

fn refused(daemon: &mut Daemon, program: Option<&std::path::Path>) -> String {
    let answer = daemon.replace(program);
    assert_eq!(answer.outcome(), proto::Outcome::Refused, "{}", answer.reason);
    answer.reason
}

#[test]
fn a_program_that_is_not_there_is_refused_and_the_daemon_goes_on() {
    let mut daemon = daemon();
    let (control, _input) = two_panes(&daemon);
    let reason = refused(&mut daemon, Some(std::path::Path::new("/nonexistent/muster-daemon")));
    assert!(reason.contains("could not start it"), "{reason}");
    still_serving(&daemon, control.welcome().instance);
}

/// A new daemon that fails at each step of a handoff costs nothing.
#[test]
fn a_new_daemon_that_refuses_or_dies_at_any_step_leaves_the_old_one_serving() {
    for (fault, said) in [
        ("refuse", "MUSTER_DAEMON_HANDOFF_FAULT=refuse"),
        ("exit-before-ready", "hung up before ready"),
        ("exit-after-commit", "hung up before serving"),
    ] {
        let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", fault)]);
        let (control, _input) = two_panes(&daemon);
        let reason = refused(&mut daemon, None);
        assert!(reason.contains(said), "{fault}: {reason}");
        still_serving(&daemon, control.welcome().instance);
        assert!(written(&daemon.root().join("daemon.log")).contains("daemon.handoff.failed"));
    }
}

/// A pane names its daemon through a link beside the socket, which the daemon taking over points
/// at itself: a pane started before a handoff to a daemon elsewhere still reaches its daemon
/// once the old one's copy is gone, as an upgrade leaves it.
#[test]
fn a_pane_reaches_its_daemon_through_muster_daemon_after_the_old_copy_is_gone() {
    // Beside the build's own directory, as deep: a debug build on macOS finds libghostty-vt by
    // a path relative to itself.
    let built = std::path::Path::new(env!("CARGO_BIN_EXE_muster-daemon"));
    let target = built.parent().and_then(std::path::Path::parent).unwrap();
    let copy = |name: &str| {
        let path =
            target.join(format!("handoff-{}-{name}", std::process::id())).join("muster-daemon");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(built, &path).unwrap();
        path
    };
    let (old, new) = (copy("old"), copy("new"));
    let mut daemon = Daemon::start(&old);
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));

    let answer = daemon.replace(Some(&new));
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    std::fs::remove_dir_all(old.parent().unwrap()).unwrap();

    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    type_line(&mut input, "p1", "\"$MUSTER_DAEMON\" report --model Linked && echo reported");
    until_text(&mut control, "p1", "\nreported");
    let record = snapshot(&mut control).panes.into_iter().find(|record| record.pane == "p1");
    let model = record.and_then(|record| record.facts).and_then(|facts| facts.model);
    assert_eq!(model.as_deref(), Some("Linked"));
    drop(daemon);
    let _ = std::fs::remove_dir_all(new.parent().unwrap());
}

/// A stop signal to the daemon handing over, at any step of a handoff, ends no pane: the handoff
/// finishes, and the old daemon exits touching nothing the new one now serves.
#[test]
fn a_stop_signal_during_a_handoff_ends_no_pane_whichever_step_it_lands_at() {
    for step in ["after-accept", "before-ready", "before-serving", "after-serving"] {
        let fault = format!("pause-{step}");
        let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", &fault)]);
        let (mut control, _input) = two_panes(&daemon);
        let pids = pids_of_both(&mut control);
        let old = daemon.pid();

        let replacing = daemon.start_replacing(None);
        daemon.paused();
        stop_signal(old);
        // Long enough for a daemon that acts on the signal at once to have hung its panes up.
        std::thread::sleep(std::time::Duration::from_millis(300));
        daemon.resume();
        let answer = daemon.finish_replacing(replacing);

        assert_eq!(answer.outcome(), proto::Outcome::Done, "{step}: {}", answer.reason);
        assert_ne!(daemon.pid(), old, "{step}: the new daemon serves");
        assert!(daemon.socket_path().exists(), "{step}: the socket is still there");
        the_same_shells_answer(&daemon, &pids, step);
    }
}

/// A stop signal waits for the handoff under way, and a handoff that then fails leaves the old
/// daemon to stop as it was asked.
#[test]
fn a_stop_signal_during_a_handoff_that_fails_stops_the_daemon_after_it() {
    let mut daemon =
        daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-ready,exit-before-ready")]);
    let (mut control, _input) = two_panes(&daemon);
    let pids = pids_of_both(&mut control);

    let replacing = daemon.start_replacing(None);
    daemon.paused();
    stop_signal(daemon.pid());
    std::thread::sleep(std::time::Duration::from_millis(300));
    let state = process_state(&pids[0]);
    assert!(!state.is_empty() && !state.starts_with('Z'), "the stop waits for the handoff");
    daemon.resume();
    let answer = daemon.finish_replacing(replacing);

    assert_eq!(answer.outcome(), proto::Outcome::Refused, "{}", answer.reason);
    daemon.wait_for_exit();
    assert!(!daemon.socket_path().exists(), "a stopped daemon removes its socket");
    for pid in &pids {
        until_some("the pane's shell to end", || process_state(pid).is_empty().then_some(()));
    }
}

/// A new daemon whose predecessor dies after the commit keeps serving: it holds every pane by
/// then, and nothing but it could.
#[test]
fn a_new_daemon_keeps_serving_when_the_old_one_dies_after_the_commit() {
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-serving")]);
    let (mut control, _input) = two_panes(&daemon);
    let pids = pids_of_both(&mut control);
    let old = daemon.pid();

    let replacing = daemon.start_replacing(None);
    let new = daemon.paused();
    // SAFETY: kill signals one process this test started.
    unsafe { libc::kill(old.cast_signed(), libc::SIGKILL) };
    let answer = daemon.finish_replacing(replacing);
    assert_ne!(answer.outcome(), proto::Outcome::Done);
    daemon.served_by(new);
    daemon.resume();

    let welcomed = daemon.connect().welcome().pid;
    assert_eq!(welcomed.cast_signed(), new, "the new daemon serves");
    the_same_shells_answer(&daemon, &pids, "after the old daemon died");
}

/// A daemon that saves nothing over its state file, because the file is a newer daemon's, hands
/// that on: the new daemon saves nothing over it either.
#[test]
fn a_new_daemon_saves_nothing_where_the_old_one_would_not() {
    let mut daemon = daemon();
    stop_signal(daemon.pid());
    daemon.wait_for_exit();
    let file = daemon.root().join("daemon.state.json");
    let newer = b"{\"version\": 999, \"tabs\": \"in a shape this daemon has never seen\"}";
    std::fs::write(&file, newer).unwrap();
    daemon.restart();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));

    replaced(&mut daemon);

    let mut control = daemon.connect();
    make(&mut control, create("p2", in_new_tab("t2")));
    // Past the persister's debounce.
    std::thread::sleep(std::time::Duration::from_secs(2));
    let now = std::fs::read(&file).unwrap();
    assert_eq!(String::from_utf8_lossy(&now), String::from_utf8_lossy(newer), "the file changed");
}

/// What `stty size` says in `name`, asked fresh.
fn tty_size(daemon: &Daemon, name: &str, ask: &str) -> String {
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    type_line(&mut input, name, &format!("echo {ask}=$(stty size | tr ' ' x)."));
    until_some(&format!("{name} to say its size"), || {
        let text = read_text(&mut control, name, 0, 0).text;
        text.lines().rev().find_map(|line| {
            let size = line.rsplit_once(&format!("{ask}="))?.1.strip_suffix('.')?;
            let digits = size.split_once('x')?;
            let numeric = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
            (numeric(digits.0) && numeric(digits.1)).then(|| size.to_string())
        })
    })
}

fn bigger() -> proto::Grid {
    proto::Grid { cols: 100, rows: 30, width_px: 0, height_px: 0 }
}

/// A bridge that resizes its pane while a handoff runs changes nothing yet: the pane the new
/// daemon takes has the size its terminal was rebuilt at, and the bridge, told REPLACED,
/// attaches again with its size.
#[test]
fn a_resize_during_a_handoff_waits_and_is_dropped_when_it_succeeds() {
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-ready")]);
    let (_control, _input) = two_panes(&daemon);
    let before = tty_size(&daemon, "p1", "before");
    let mut stream = attached(&daemon, "p1", false);

    let replacing = daemon.start_replacing(None);
    daemon.paused();
    stream.resize(bigger());
    std::thread::sleep(std::time::Duration::from_millis(300));
    daemon.resume();
    let answer = daemon.finish_replacing(replacing);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    assert_eq!(tty_size(&daemon, "p1", "after"), before);
}

/// The same resize, when the handoff fails, takes effect once the old daemon goes on.
#[test]
fn a_resize_during_a_handoff_that_fails_takes_effect_after_it() {
    let mut daemon =
        daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-ready,exit-before-ready")]);
    let (_control, _input) = two_panes(&daemon);
    let mut stream = attached(&daemon, "p1", false);

    let replacing = daemon.start_replacing(None);
    daemon.paused();
    stream.resize(bigger());
    std::thread::sleep(std::time::Duration::from_millis(300));
    daemon.resume();
    let answer = daemon.finish_replacing(replacing);
    assert_eq!(answer.outcome(), proto::Outcome::Refused, "{}", answer.reason);

    assert_eq!(tty_size(&daemon, "p1", "after"), "30x100");
}
