//! Starting a pane, what it runs, and how it ends.

use std::collections::HashMap;

use crate::support::*;
use proto::Side;

#[test]
fn a_pane_in_a_new_tab_is_announced_before_its_answer() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let subscribed = expect(&mut control, subscribe_request(), proto::Outcome::Done);
    let before = subscribed.answer.seq;

    let asked = make(&mut control, create("p1", in_new_tab("t1")));
    assert_eq!(names(&asked.events), ["pane_opened:p1", "tab_opened:t1"]);
    let numbers: Vec<u64> = asked.events.iter().map(|event| event.seq).collect();
    assert_eq!(numbers, [before + 1, before + 2]);
    assert_eq!(asked.answer.seq, before + 2, "the answer names the last event it produced");
    assert_eq!(tab_shape(&mut control, "t1"), "p1");
}

#[test]
fn a_pane_goes_on_the_side_asked_and_its_neighbour_keeps_the_share_asked() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("a", in_new_tab("t1")));
    let mut right = create("b", beside("a", Side::Right));
    if let Some(proto::placement::Where::Beside(beside)) =
        right.placement.as_mut().and_then(|placement| placement.r#where.as_mut())
    {
        beside.ratio = Some(0.7);
    }
    make(&mut control, right);
    assert_eq!(tab_shape(&mut control, "t1"), "[a|b 0.70]");
    make(&mut control, create("c", beside("b", Side::Up)));
    assert_eq!(tab_shape(&mut control, "t1"), "[a|[c/b 0.50] 0.70]");
    make(&mut control, create("d", beside("a", Side::Left)));
    make(&mut control, create("e", beside("a", Side::Down)));
    assert_eq!(tab_shape(&mut control, "t1"), "[[d|[a/e 0.50] 0.50]|[c/b 0.50] 0.70]");
}

#[test]
fn a_create_repeated_or_placed_nowhere_says_so() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let again = expect(
        &mut control,
        create_request(create("p1", in_new_tab("t2"))),
        proto::Outcome::AlreadySo,
    );
    assert!(again.events.is_empty(), "a repeated create starts nothing");
    expect(
        &mut control,
        create_request(create("p2", beside("missing", Side::Right))),
        proto::Outcome::NotThere,
    );
    expect(&mut control, create_request(create("p2", in_new_tab("t1"))), proto::Outcome::Refused);
    expect(
        &mut control,
        create_request(create("has space", in_new_tab("t3"))),
        proto::Outcome::Refused,
    );
    assert_eq!(snapshot(&mut control).panes.len(), 1);
}

#[test]
fn a_pane_is_told_its_name_and_its_window_and_nothing_stale() {
    let daemon = daemon_with(&[("MUSTER_PANE", "p-stale"), ("MUSTER_SOCKET", "/stale.sock")]);
    let mut control = daemon.connect();
    let out = daemon.root().join("env");
    let mut asked = create("p1", in_new_tab("t1"));
    asked.env = HashMap::from([("MUSTER_SOCKET".to_string(), "/window.sock".to_string())]);
    asked.command = Some(format!("env > {}", out.display()));
    make(&mut control, asked);

    let environment = written(&out);
    let lines: Vec<&str> = environment.lines().collect();
    for expected in ["MUSTER_PANE=p1", "MUSTER_SOCKET=/window.sock", "COLORTERM=truecolor"] {
        assert!(lines.contains(&expected), "{expected} missing from:\n{environment}");
    }
    assert!(
        !environment.contains("stale"),
        "an inherited name leaked into the pane:\n{environment}"
    );
}

#[test]
fn a_command_runs_and_the_pane_becomes_its_shell_afterwards() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let pid = daemon.root().join("pid");
    let mut asked = create("p1", in_new_tab("t1"));
    asked.command = Some(format!("echo $$ > {}", pid.display()));
    make(&mut control, asked);

    let pid = written(&pid);
    // The same process, having replaced itself with the interactive shell the pane drops to.
    until(
        "the command's shell to exec an interactive shell",
        || process_state(&pid).ends_with("/bin/sh -l -i"),
        || format!("ps says: {:?}", process_state(&pid)),
    );
    assert_eq!(snapshot(&mut control).panes.len(), 1);
}

/// A command the shell cannot parse fails as a shell error, and the pane still drops to its
/// shell rather than closing. Every login shell sources `~/.profile`, so the shell that ran the
/// command and the one it became each leave a line.
#[test]
fn a_command_the_shell_cannot_parse_still_leaves_a_shell() {
    let daemon = daemon();
    let shells = daemon.root().join("shells");
    std::fs::write(
        daemon.root().join("home/.profile"),
        format!("echo $$ >> {}\n", shells.display()),
    )
    .unwrap();
    let mut control = daemon.connect();
    for (name, command) in
        [("quote", "echo 'unclosed"), ("backslash", "echo \\"), ("heredoc", "cat <<END")]
    {
        let _ = std::fs::remove_file(&shells);
        let mut asked = create(name, in_new_tab(name));
        asked.command = Some(command.to_string());
        make(&mut control, asked);
        until(
            &format!("the pane running {command:?} to become its shell"),
            || std::fs::read_to_string(&shells).is_ok_and(|started| started.lines().count() == 2),
            || format!("shells started: {:?}", std::fs::read_to_string(&shells).ok()),
        );
        assert!(snapshot(&mut control).panes.iter().any(|pane| pane.pane == name));
    }
}

#[test]
fn a_pane_is_its_grid_from_birth_and_a_split_inherits_its_neighbours() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let first = daemon.root().join("first");
    let mut asked = create("p1", in_new_tab("t1"));
    asked.grid = Some(proto::Grid { cols: 100, rows: 30, width_px: 800, height_px: 600 });
    asked.command = Some(format!("stty size > {}", first.display()));
    make(&mut control, asked);
    assert_eq!(written(&first), "30 100\n");

    let second = daemon.root().join("second");
    let mut asked = create("p2", beside("p1", Side::Down));
    asked.command = Some(format!("stty size > {}", second.display()));
    make(&mut control, asked);
    assert_eq!(written(&second), "30 100\n");

    let mut asked = create("p3", in_new_tab("t2"));
    asked.grid = Some(proto::Grid { cols: 0, rows: 30, width_px: 0, height_px: 0 });
    expect(&mut control, create_request(asked), proto::Outcome::Refused);
}

#[test]
fn a_pane_starts_where_asked_or_where_its_neighbour_is_now() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let asked_for = daemon.root().join("asked-for");
    let wandered = daemon.root().join("wandered");
    std::fs::create_dir_all(&asked_for).unwrap();
    std::fs::create_dir_all(&wandered).unwrap();

    let marker = daemon.root().join("moved");
    let mut first = create("p1", in_new_tab("t1"));
    first.cwd = Some(asked_for.display().to_string());
    first.command = Some(format!(
        "pwd > {} && cd {} && echo > {}",
        asked_for.join("pwd").display(),
        wandered.display(),
        marker.display()
    ));
    make(&mut control, first);
    assert_eq!(written(&asked_for.join("pwd")).trim(), canonical(&asked_for).display().to_string());
    written(&marker);

    let out = daemon.root().join("inherited");
    let mut second = create("p2", beside("p1", Side::Right));
    second.command = Some(format!("pwd > {}", out.display()));
    make(&mut control, second);
    assert_eq!(written(&out).trim(), canonical(&wandered).display().to_string());
}

#[test]
fn a_pane_inherits_no_other_panes_descriptors() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    assert_eq!(
        descriptors_of_a_new_pane(&daemon, &mut control, beside("p1", Side::Right)),
        "end\n"
    );
}

/// Whatever the daemon holds open, a pane gets its terminal and nothing else - including a
/// descriptor some other thread opened a moment before the fork and had not yet marked
/// close-on-exec, which std's `accept` on macOS leaves open for exactly that moment. A
/// descriptor the daemon inherited without the flag stands in for it, deterministically.
#[test]
fn a_descriptor_the_daemon_holds_without_close_on_exec_never_reaches_a_pane() {
    let daemon = daemon_holding(9);
    let mut control = daemon.connect();
    assert_eq!(descriptors_of_a_new_pane(&daemon, &mut control, in_new_tab("t1")), "end\n");
}

/// Starts a pane that lists every descriptor it holds above stderr, and returns the list.
fn descriptors_of_a_new_pane(
    daemon: &Daemon,
    control: &mut Control,
    placement: proto::Placement,
) -> String {
    // bash by name, because the loop below knows where bash keeps its own descriptors and
    // /bin/sh is not bash everywhere: Debian's dash keeps its terminal on descriptor 10. Not a
    // login shell: bash started with both -l and -i closes descriptors it inherited, which would
    // hide a leak from this test while a person's zsh kept it.
    use_shell(control, "/bin/bash", proto::ShellMode::NonLogin);

    let out = daemon.root().join("fds");
    let mut asked = create("listing", placement);
    // The shell's own test builtin asks about each descriptor, so nothing it runs opens one of
    // its own while it looks - including bash, which parks stdout on descriptor 10 while a
    // redirection is in force, so each line is appended rather than the loop redirected. 255 is
    // where an interactive bash keeps its terminal.
    asked.command = Some(format!(
        "i=3; while [ $i -lt 255 ]; do [ -e /dev/fd/$i ] && echo $i >> {out}; i=$((i+1)); done; \
         echo end >> {out}",
        out = out.display()
    ));
    make(control, asked);
    until(
        "the pane to finish listing its descriptors",
        || std::fs::read_to_string(&out).is_ok_and(|listed| listed.ends_with("end\n")),
        (),
    );
    std::fs::read_to_string(&out).unwrap()
}

#[test]
fn closing_a_pane_hangs_up_its_processes_and_reaps_them() {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    // bash by name: an interactive dash, Debian's /bin/sh, holds a HUP trap until a loop it is
    // running ends, and this loop never does.
    use_shell(&mut control, "/bin/bash", proto::ShellMode::Login);
    make(&mut control, create("p1", in_new_tab("t1")));
    let pid = daemon.root().join("pid");
    let hup = daemon.root().join("hup");
    let mut asked = create("p2", beside("p1", Side::Right));
    asked.command = Some(format!(
        "trap 'echo hup > {}; exit' HUP; echo $$ > {}; while :; do sleep 1; done",
        hup.display(),
        pid.display()
    ));
    make(&mut control, asked);
    let pid = written(&pid);

    let closed = expect(&mut control, close_request("p2"), proto::Outcome::Done);
    // p1 is an integrated bash, which titles itself after each prompt whenever it gets there.
    let mut published = names(&closed.events);
    published.retain(|name| name != "pane_changed:p1");
    assert_eq!(published, ["tab_changed:t1", "pane_closed:p2"]);
    assert_eq!(written(&hup), "hup\n");
    until(
        "the closed pane's shell to be reaped",
        || process_state(&pid).is_empty(),
        || format!("ps still says: {:?}", process_state(&pid)),
    );
    expect(&mut control, close_request("p2"), proto::Outcome::NotThere);
}

#[test]
fn a_pane_whose_process_exits_closes_and_says_how() {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    let mut asked = create("p1", in_new_tab("t1"));
    asked.command = Some("exit 3".to_string());
    control.send(create_request(asked));

    let mut seen = Vec::new();
    let closed = until_some("the pane to close", || {
        match control.next_message(muster_harness::PATIENCE)? {
            proto::control_message::Message::Event(event) => {
                seen.push(named(&event));
                match event.event {
                    Some(proto::event::Event::PaneClosed(closed)) => Some(closed),
                    _ => None,
                }
            }
            proto::control_message::Message::Answer(_)
            | proto::control_message::Message::LogLine(_) => None,
        }
    });
    assert_eq!(seen, ["pane_opened:p1", "tab_opened:t1", "tab_closed:t1", "pane_closed:p1"]);
    assert_eq!(closed.reason(), proto::CloseReason::Exited);
    assert_eq!(closed.exit_status, Some(3));

    // A daemon with nothing in it is still a daemon.
    assert!(snapshot(&mut control).tabs.is_empty());
    make(&mut control, create("p2", in_new_tab("t2")));
}

#[test]
fn a_pane_that_cannot_start_is_refused_and_leaves_nothing_behind() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let mut asked = create("p1", in_new_tab("t1"));
    asked.cwd = Some("/no/such/directory".to_string());
    let refused = expect(&mut control, create_request(asked), proto::Outcome::Refused);
    assert!(refused.answer.reason.contains("/no/such/directory"), "{}", refused.answer.reason);
    assert!(snapshot(&mut control).tabs.is_empty());
}

/// Every pane after this runs `command` as its shell, in `mode`.
fn use_shell(control: &mut Control, command: &str, mode: proto::ShellMode) {
    let shell = proto::Shell {
        command: Some(command.to_string()),
        mode: mode.into(),
        ..proto::Shell::default()
    };
    let set = proto::SetShell { shell: Some(shell) };
    let asked = control.ask(session(proto::session_request::Request::SetShell(set)));
    assert!(matches!(asked.outcome(), proto::Outcome::Done | proto::Outcome::AlreadySo));
}

/// A test that types into a pane and reads it back depends on where the prompt ends, and the
/// system's prompt names the machine: macOS's is `host:dir user$`. On a runner whose hostname is
/// 62 characters long, text typed after it reached the 80th column, wrapped, and read back split
/// in two. So every pane a test starts has the same prompt, wherever the suite runs.
#[test]
fn a_panes_prompt_is_the_same_on_every_machine() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let text = until_text(&mut control, "p1", "$");
    assert_eq!(text.trim_end(), "$", "the prompt names the machine it runs on");
}
