//! Replacing a running daemon (MIP-3 section 10): every pane goes to the new daemon with its
//! process, its screen and its place, and a handoff that fails anywhere leaves the old daemon
//! serving as it was.

use crate::support::*;
use muster_harness::Input;
use proto::input_event::{self, Input as Event};
use proto::{pane_request, session_request, tab_request};

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
    // Settled first: a shell's directory, as the kernel names it, can still be on its way.
    let before = until_some("the pane records to settle", || {
        let first = snapshot(&mut control);
        std::thread::sleep(std::time::Duration::from_millis(300));
        (snapshot(&mut control).panes == first.panes).then_some(first)
    });
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

/// The log's file holds the old daemon's records up to its commit, then the new one's, from its
/// first: none lost, and never the two interleaved.
#[test]
fn the_log_holds_the_old_daemon_up_to_its_commit_and_then_the_new_one() {
    let mut daemon = daemon();
    let (_control, _input) = two_panes(&daemon);
    let old = daemon.pid();

    replaced(&mut daemon);
    let new = daemon.pid();

    let log = until_some("the new daemon's records", || {
        let log = std::fs::read_to_string(daemon.root().join("daemon.log")).ok()?;
        log.contains("daemon.handoff.serving").then_some(log)
    });
    let records: Vec<(u32, &str)> = log
        .lines()
        .map(|line| {
            let pid = line
                .split("\"pid\":")
                .nth(1)
                .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok());
            let event = line.split("\"event\":\"").nth(1).and_then(|rest| rest.split('"').next());
            (pid.expect("every record has a pid"), event.expect("every record has an event"))
        })
        .collect();
    let first_new = records.iter().position(|(pid, _)| *pid == new).expect("the new daemon's");
    let (before, after) = records.split_at(first_new);
    assert!(before.iter().all(|(pid, _)| *pid == old), "only the old daemon before: {log}");
    assert!(after.iter().all(|(pid, _)| *pid == new), "only the new daemon after: {log}");
    let events = |records: &[(u32, &str)]| records.iter().map(|(_, e)| e.to_string()).collect();
    let old_events: Vec<String> = events(before);
    assert_eq!(old_events.first().map(String::as_str), Some("daemon.started"));
    assert_eq!(old_events.last().map(String::as_str), Some("daemon.handoff.committed"));
    let new_events: Vec<String> = events(after);
    assert_eq!(
        new_events.first().map(String::as_str),
        Some("daemon.handoff.taking_over"),
        "the new daemon's records from its first, kept until the commit"
    );
    assert!(new_events.iter().any(|event| event == "daemon.handoff.serving"));
}

/// A connection made while the old daemon has stopped accepting waits for whichever daemon
/// serves next: the new one, once the handoff succeeds.
#[test]
fn a_connection_made_during_a_handoff_is_served_by_the_new_daemon() {
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-ready")]);
    let (_control, _input) = two_panes(&daemon);

    let replacing = daemon.start_replacing(None);
    let new = daemon.paused();
    let socket = daemon.socket_path().to_path_buf();
    let waiting = std::thread::spawn(move || Control::connect(&socket).welcome().pid);
    std::thread::sleep(std::time::Duration::from_millis(200));
    daemon.resume();
    let answer = daemon.finish_replacing(replacing);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    assert_eq!(waiting.join().unwrap().cast_signed(), new);
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

/// A program is run once with `--version` before any pane is touched, and one that does not
/// answer in time, or answers with a failure, is refused there.
#[test]
fn a_program_that_does_not_answer_its_version_is_refused_before_anything_is_touched() {
    let scripts = std::env::temp_dir().join(format!("muster-launch-{}", std::process::id()));
    std::fs::create_dir_all(&scripts).unwrap();
    for (name, body, said) in [
        ("slow", "exec sleep 10", "did not answer --version within 1 s"),
        ("failing", "exit 3", "answered --version with exit status: 3"),
    ] {
        let program = scripts.join(name);
        std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "short-launch")]);
        let (control, _input) = two_panes(&daemon);
        let asked = std::time::Instant::now();
        let reason = refused(&mut daemon, Some(&program));
        assert!(reason.contains(said), "{name}: {reason}");
        assert!(
            asked.elapsed() < std::time::Duration::from_secs(5),
            "{name}: waited {:?}",
            asked.elapsed()
        );
        still_serving(&daemon, control.welcome().instance);
    }
    let _ = std::fs::remove_dir_all(&scripts);
}

/// A stand-in for a new daemon that, asked its `--version`, says so and waits to be let go, then
/// exits with `status`: a first run of a new binary that takes as long as a test needs.
struct HeldAtVersion {
    scripts: std::path::PathBuf,
    program: std::path::PathBuf,
}

impl HeldAtVersion {
    fn new(name: &str, status: u8) -> HeldAtVersion {
        let scripts = std::env::temp_dir().join(format!("muster-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&scripts).unwrap();
        let program = scripts.join("muster-daemon");
        let script = format!(
            // Bounded, so a test that fails before it lets go leaves nothing running.
            "#!/bin/sh\ntouch '{}'\nn=0\nwhile [ ! -e '{}' ] && [ $n -lt 600 ]; do\n\
             sleep 0.05; n=$((n + 1))\ndone\nexit {status}\n",
            scripts.join("asked").display(),
            scripts.join("go").display()
        );
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        HeldAtVersion { scripts, program }
    }

    fn until_asked(&self) {
        let asked = self.scripts.join("asked");
        until_some("the program to be asked its --version", || asked.exists().then_some(()));
    }

    fn let_go(&self) {
        std::fs::write(self.scripts.join("go"), "").unwrap();
    }
}

impl Drop for HeldAtVersion {
    fn drop(&mut self) {
        self.let_go();
        let _ = std::fs::remove_dir_all(&self.scripts);
    }
}

/// The first run of a new binary can take seconds on macOS, and the daemon goes on serving
/// while it waits for the program's `--version`: changes are refused only once the panes are
/// being handed over.
#[test]
fn the_daemon_goes_on_taking_changes_while_it_waits_for_the_version() {
    let held = HeldAtVersion::new("waiting", 1);
    let mut daemon = daemon();
    let (mut control, _input) = two_panes(&daemon);

    let replacing = daemon.start_replacing(Some(&held.program));
    held.until_asked();
    let label = pane_request::Rename { pane: "p1".to_string(), label: Some("A".to_string()) };
    expect(&mut control, pane(pane_request::Request::Rename(label)), proto::Outcome::Done);
    make(&mut control, create("p4", in_new_tab("t4")));
    held.let_go();
    let answer = daemon.finish_replacing(replacing);

    assert_eq!(answer.outcome(), proto::Outcome::Refused, "{}", answer.reason);
    assert!(answer.reason.contains("exit status: 1"), "{}", answer.reason);
    still_serving(&daemon, control.welcome().instance);
}

/// Nothing has been handed over while the program is asked its version, so a stop signal then
/// stops the daemon as at any other time. The request is never answered: the daemon has gone.
#[test]
fn a_stop_signal_while_the_version_is_asked_stops_the_daemon_at_once() {
    let held = HeldAtVersion::new("stopping", 1);
    let mut daemon = daemon();
    let (mut control, _input) = two_panes(&daemon);
    let pids = pids_of_both(&mut control);

    let replacing = daemon.start_replacing(Some(&held.program));
    held.until_asked();
    stop_signal(daemon.pid());
    until_some("the panes' shells to end", || {
        pids.iter()
            .all(|pid| {
                let state = process_state(pid);
                state.is_empty() || state.starts_with('Z')
            })
            .then_some(())
    });
    daemon.wait_for_exit();
    assert!(!daemon.socket_path().exists(), "a daemon that stopped removes its socket");
    held.let_go();
    let answer = daemon.finish_replacing(replacing);
    assert_eq!(answer.reason, "the daemon hung up without answering");
}

/// A replace asked while another is under way is refused, and says so: nothing failed, and the
/// handoff under way goes on.
#[test]
fn a_replace_asked_during_another_is_refused_as_such_and_the_first_goes_on() {
    let held = HeldAtVersion::new("second", 0);
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-hold")]);
    let (mut control, _input) = two_panes(&daemon);
    let pids = pids_of_both(&mut control);

    let second = daemon.start_replacing(Some(&held.program));
    held.until_asked();
    let first = daemon.start_replacing(None);
    daemon.paused();
    held.let_go();
    let refused = daemon.finish_replacing(second);
    assert_eq!(refused.outcome(), proto::Outcome::Refused, "{}", refused.reason);
    assert!(refused.reason.contains("already being replaced"), "{}", refused.reason);
    daemon.resume();
    let answer = daemon.finish_replacing(first);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    the_same_shells_answer(&daemon, &pids, "after the first handoff");

    let log = written(&daemon.root().join("daemon.log"));
    let line = log.lines().find(|line| line.contains("already being replaced")).expect("logged");
    assert!(!line.contains("daemon.handoff.failed"), "logged as a failure: {line}");
}

/// The program asked its version gets none of the daemon's descriptors, as the successor does
/// not: now that panes are made while it runs, one could be forked from between a terminal's
/// opening and its close-on-exec. A descriptor the daemon holds without the flag stands in.
#[test]
fn the_program_asked_its_version_holds_none_of_the_daemons_descriptors() {
    let scripts = std::env::temp_dir().join(format!("muster-sealed-{}", std::process::id()));
    std::fs::create_dir_all(&scripts).unwrap();
    let (program, out) = (scripts.join("muster-daemon"), scripts.join("fds"));
    // bash by name, as for a pane's own listing; 255 is where bash keeps the script it reads.
    let script = format!(
        "#!/bin/bash\ni=3; while [ $i -lt 255 ]; do [ -e /dev/fd/$i ] && echo $i >> '{out}'; \
         i=$((i+1)); done; echo end >> '{out}'\nexit 1\n",
        out = out.display()
    );
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .unwrap();
    let mut daemon = daemon_holding(9);
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));

    refused(&mut daemon, Some(&program));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "end\n");
    let _ = std::fs::remove_dir_all(&scripts);
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

/// Copies of the daemon beside the build's own directory, as deep: a debug build on macOS finds
/// libghostty-vt by a path relative to itself.
fn copies_of_the_daemon(names: [&str; 2]) -> [std::path::PathBuf; 2] {
    let built = std::path::Path::new(env!("CARGO_BIN_EXE_muster-daemon"));
    let target = built.parent().and_then(std::path::Path::parent).unwrap();
    names.map(|name| {
        let path =
            target.join(format!("handoff-{}-{name}", std::process::id())).join("muster-daemon");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(built, &path).unwrap();
        path
    })
}

/// A new daemon that dies after pointing the link at itself leaves it pointing back at the old
/// daemon, which goes on serving.
#[test]
fn a_failed_handoff_points_the_link_back_at_the_old_daemon() {
    let [old, new] = copies_of_the_daemon(["link-old", "link-new"]);
    let mut daemon =
        Daemon::start_with(&old, &[("MUSTER_DAEMON_HANDOFF_FAULT", "exit-after-commit")]);
    let link = daemon.root().join("daemon.muster-daemon");
    assert_eq!(std::fs::read_link(&link).unwrap(), old);

    let answer = daemon.replace(Some(&new));
    assert_eq!(answer.outcome(), proto::Outcome::Refused, "{}", answer.reason);

    assert_eq!(std::fs::read_link(&link).unwrap(), old);
    drop(daemon);
    for copy in [old, new] {
        let _ = std::fs::remove_dir_all(copy.parent().unwrap());
    }
}

/// A pane names its daemon through a link beside the socket, which the daemon taking over points
/// at itself: a pane started before a handoff to a daemon elsewhere still reaches its daemon
/// once the old one's copy is gone, as an upgrade leaves it.
#[test]
fn a_pane_reaches_its_daemon_through_muster_daemon_after_the_old_copy_is_gone() {
    let [old, new] = copies_of_the_daemon(["old", "new"]);
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

/// A report queued as the last reader parks is applied under the session's lock, so the
/// handoff waits for it without holding that lock, and neither it nor any request stalls.
#[test]
fn a_report_queued_as_the_readers_stop_does_not_stall_the_handoff() {
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "report-before-settle")]);
    let (_control, _input) = two_panes(&daemon);

    let started = std::time::Instant::now();
    replaced(&mut daemon);
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(5), "the handoff took {took:?}");
}

/// A pane whose shell exits once the handoff is under way is gone from the old daemon's tabs,
/// so it cannot be handed over in its place: the handoff fails, and the old daemon goes on.
#[test]
fn a_pane_that_ends_during_a_handoff_fails_it() {
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-hold")]);
    let (mut control, _input) = two_panes(&daemon);
    let pid: i32 = pids_of_both(&mut control)[1].parse().unwrap();
    let instance = control.welcome().instance;

    let replacing = daemon.start_replacing(None);
    daemon.paused();
    // SAFETY: kill signals the shell of a pane this test made.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
    until("p2 to end", || !snapshot(&mut control).panes.iter().any(|pane| pane.pane == "p2"), ());
    daemon.resume();
    let answer = daemon.finish_replacing(replacing);

    assert_eq!(answer.outcome(), proto::Outcome::Refused, "{}", answer.reason);
    assert!(answer.reason.contains("p2"), "{}", answer.reason);
    still_serving(&daemon, instance);
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

fn clear_screen(input: &mut Input, pane: &str) {
    let clear = input_event::perform::Action::ClearScreen(input_event::perform::ClearScreen {});
    input.send(
        pane,
        Event::Perform(input_event::Perform { action: Some(clear), ..Default::default() }),
    );
}

/// A pane with history, attached to a surface that has drawn it.
fn drawn_with_history(daemon: &Daemon, input: &mut Input) -> (Stream, Surface) {
    let mut control = daemon.connect();
    type_line(input, "p1", "seq 1 60");
    until_text(&mut control, "p1", "\n60\n");
    let mut stream = attached(daemon, "p1", false);
    let mut surface = Surface::new(100, 30);
    surface.follow(&mut stream, "the replay", true, |surface| surface.replays > 0);
    (stream, surface)
}

/// clear_screen asked of a pane while it is held waits, as a resize does: its replay may be
/// composed already, and a clear the new daemon never hears of would come back at the next
/// attach. When the handoff succeeds the clear is dropped, and the surface is left as it was.
#[test]
fn clear_screen_during_a_handoff_waits_and_is_dropped_when_it_succeeds() {
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-ready")]);
    let (_control, mut input) = two_panes(&daemon);
    let (mut stream, mut surface) = drawn_with_history(&daemon, &mut input);

    let replacing = daemon.start_replacing(None);
    daemon.paused();
    clear_screen(&mut input, "p1");
    std::thread::sleep(std::time::Duration::from_millis(300));
    daemon.resume();
    let answer = daemon.finish_replacing(replacing);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    surface.follow(&mut stream, "the detach", true, |surface| surface.detached.is_some());
    assert!(surface.text().contains("\n59\n"), "the surface was not cleared: {}", surface.text());
    let mut control = daemon.connect();
    assert!(read_text(&mut control, "p1", 0, 0).text.contains("\n59\n"), "nor the new terminal");
}

/// The same clear, when the handoff fails, is done once the old daemon goes on.
#[test]
fn clear_screen_during_a_handoff_that_fails_is_done_after_it() {
    let mut daemon =
        daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-ready,exit-before-ready")]);
    let (mut control, mut input) = two_panes(&daemon);
    let (_stream, _surface) = drawn_with_history(&daemon, &mut input);

    let replacing = daemon.start_replacing(None);
    daemon.paused();
    clear_screen(&mut input, "p1");
    std::thread::sleep(std::time::Duration::from_millis(300));
    daemon.resume();
    let answer = daemon.finish_replacing(replacing);
    assert_eq!(answer.outcome(), proto::Outcome::Refused, "{}", answer.reason);

    until("the history to go", || !read_text(&mut control, "p1", 0, 0).text.contains("\n59\n"), ());
}

/// On the alternate screen that clear is the key, sent to the program, and it still reaches it
/// when the clear waited out a handoff that failed.
#[test]
fn clear_screen_on_the_alternate_screen_during_a_handoff_that_fails_sends_the_key() {
    const KEY_K: u32 = 30;
    let mut daemon =
        daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-ready,exit-before-ready")]);
    let mut control = daemon.connect();
    let heard = daemon.root().join("heard");
    let script = format!(
        "printf '\\033[?1049hvim'; stty raw -echo min 1 time 0; \
         dd bs=1 count=1 of={} 2>/dev/null; sleep 30",
        heard.display()
    );
    let vim = pane_request::Create { command: Some(script), ..create("p1", in_new_tab("t1")) };
    make(&mut control, vim);
    until_text(&mut control, "p1", "vim");
    let mut input = Input::connect(daemon.socket_path());

    let replacing = daemon.start_replacing(None);
    daemon.paused();
    let clear = input_event::perform::Action::ClearScreen(input_event::perform::ClearScreen {});
    let key = input_event::Key {
        action: proto::KeyAction::Press.into(),
        key: KEY_K,
        text: "k".to_string(),
        unshifted_codepoint: u32::from('k'),
        ..input_event::Key::default()
    };
    input.send("p1", Event::Perform(input_event::Perform { action: Some(clear), key: Some(key) }));
    std::thread::sleep(std::time::Duration::from_millis(300));
    daemon.resume();
    let answer = daemon.finish_replacing(replacing);
    assert_eq!(answer.outcome(), proto::Outcome::Refused, "{}", answer.reason);

    assert_eq!(bytes_in(&heard), b"k", "the program gets the key");
}

/// A new daemon that fails before the commit never wrote the log's file, and what it logged is
/// still there afterwards: the old daemon writes it in, ahead of its own word on the failure.
#[test]
fn a_new_daemon_that_fails_before_the_commit_leaves_its_records_in_the_log() {
    for fault in ["refuse", "exit-before-ready"] {
        let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", fault)]);
        let (_control, _input) = two_panes(&daemon);
        let old = format!("\"pid\":{},", daemon.pid());
        refused(&mut daemon, None);

        let log = written(&daemon.root().join("daemon.log"));
        let taking = log
            .lines()
            .position(|line| line.contains("daemon.handoff.taking_over"))
            .unwrap_or_else(|| panic!("{fault}: the new daemon's records are not in:\n{log}"));
        assert!(!log.lines().nth(taking).unwrap().contains(&old), "{fault}: the new daemon's");
        let failed = log.lines().position(|line| line.contains("daemon.handoff.failed")).unwrap();
        assert!(taking < failed, "{fault}: its records come before the failure");
        assert!(!daemon.root().join("daemon.log.handoff").exists(), "{fault}: nothing left");
    }
}

/// A pane closed just before a handoff, whose program ignores the hang-up, is still killed: the
/// daemon handing over finishes the kill before it exits.
#[test]
fn a_kill_under_way_at_a_handoff_still_happens() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let deaf = pane_request::Create {
        command: Some("echo pid=$$.; trap '' HUP; while :; do sleep 1; done".to_string()),
        ..create("p9", in_new_tab("t9"))
    };
    make(&mut control, deaf);
    let pid = until_said(&mut control, "p9", "pid");
    expect(&mut control, close_request("p9"), proto::Outcome::Done);

    replaced(&mut daemon);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    while !process_state(&pid).is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let left = process_state(&pid);
    if !left.is_empty() {
        let _ = std::process::Command::new("kill").args(["-9", &pid]).status();
    }
    assert_eq!(left, "", "the closed pane's program outlived the handoff");
}

/// An adopted pane, closed while its program ignores the hang-up, is killed after the grace as
/// a pane of the new daemon's own would be: its program still leads the session its terminal
/// belongs to, which is what lets the new daemon signal a group it did not start.
#[test]
fn an_adopted_pane_whose_program_ignores_the_hang_up_is_killed_when_closed() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let deaf = pane_request::Create {
        command: Some("echo pid=$$.; trap '' HUP; while :; do sleep 1; done".to_string()),
        ..create("p9", in_new_tab("t9"))
    };
    make(&mut control, deaf);
    let pid = until_said(&mut control, "p9", "pid");

    replaced(&mut daemon);
    let mut control = daemon.connect();
    expect(&mut control, close_request("p9"), proto::Outcome::Done);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    while !process_state(&pid).is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let left = process_state(&pid);
    if !left.is_empty() {
        let _ = std::process::Command::new("kill").args(["-9", &pid]).status();
    }
    assert_eq!(left, "", "the closed adopted pane's program outlived its close");
}
