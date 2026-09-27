//! What a pane's programs are told about the terminal they run in (MIP-3 section 3).

mod support;

use support::*;

#[test]
fn a_pane_runs_as_xterm_ghostty_from_the_daemons_own_terminfo() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let out = daemon.root().join("infocmp");
    let mut asked = create("p1", in_new_tab("t1"));
    asked.command = Some(format!(
        "{{ infocmp -x; echo \"$TERM_PROGRAM $TERM_PROGRAM_VERSION\"; echo end; }} > {} 2>&1",
        out.display()
    ));
    make(&mut control, asked);
    let written = until_some("the pane to describe its terminal", || {
        std::fs::read_to_string(&out).ok().filter(|text| text.ends_with("end\n"))
    });

    // The first line says which file the entry came from: the daemon's, and not a system copy
    // that happens to exist on this machine.
    let data = std::path::Path::new(DAEMON_DATA).canonicalize().unwrap();
    let source = written.lines().next().unwrap_or_default();
    assert!(
        source.contains(&data.join("terminfo").display().to_string()),
        "infocmp read {source:?}\n{written}"
    );
    assert!(written.contains("\nxterm-ghostty|ghostty|Ghostty,\n"), "{written}");
    assert!(written.contains("\nghostty "), "TERM_PROGRAM: {written}");
}

#[test]
fn a_daemon_without_its_data_directory_refuses_to_start() {
    let scratch = std::env::temp_dir().join(format!("muster-no-data-{}", std::process::id()));
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_muster-daemon"))
        .arg("--socket")
        .arg(scratch.join("daemon.sock"))
        .arg("--data")
        .arg(&scratch)
        .env_clear()
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "it started: {stderr}");
    assert!(stderr.contains(&format!("{} is not a complete", scratch.display())), "{stderr}");
    assert!(!scratch.join("daemon.sock").exists(), "it claimed the socket anyway");
}

/// Where `name` is on this machine's PATH, leaving out Apple's /bin/bash, which Ghostty does not
/// integrate with.
fn installed(name: &str) -> Option<String> {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|dir| format!("{dir}/{name}"))
        .filter(|path| !(cfg!(target_os = "macos") && path == "/bin/bash"))
        .find(|path| std::path::Path::new(path).is_file())
}

/// Starts `shell` in a pane, running `command` first if given, and waits for a prompt that
/// carries OSC 133's prompt-start mark, which only Ghostty's integration puts there.
fn prompts_are_marked(name: &str, required: bool, command: Option<&str>) {
    let Some(shell) = installed(name) else {
        assert!(!required, "{name} is not installed, and this machine is expected to have it");
        eprintln!("skipped: {name} is not installed here; ./dev --linux runs this with it");
        return;
    };
    let daemon = daemon();
    // zsh with no startup file of its own offers to write one instead of prompting.
    std::fs::write(daemon.root().join("home/.zshrc"), "").unwrap();
    let mut control = daemon.connect();
    let set = proto::SetShell {
        shell: Some(proto::Shell {
            command: Some(shell.clone()),
            mode: proto::ShellMode::Login.into(),
        }),
    };
    expect(
        &mut control,
        session(proto::session_request::Request::SetShell(set)),
        proto::Outcome::Done,
    );
    make(
        &mut control,
        proto::pane_request::Create {
            grid: Some(proto::Grid { cols: 80, rows: 24, width_px: 800, height_px: 480 }),
            command: command.map(str::to_string),
            ..create("p1", in_new_tab("t1"))
        },
    );

    // The first prompt may be drawn before the stream attaches, and a replay carries the screen
    // rather than the marks, so the test asks for fresh prompts until one arrives as output.
    let mut stream = attached(&daemon, "p1", false);
    let mut input = muster_harness::Input::connect(daemon.socket_path());
    let enter = || {
        proto::input_event::Input::Send(proto::input_event::Send {
            text: String::new(),
            enter: true,
        })
    };
    let mut output = Vec::new();
    let deadline = std::time::Instant::now() + muster_harness::PATIENCE;
    let mut next_enter = std::time::Instant::now();
    while !output.windows(7).any(|window| window == b"\x1b]133;A") {
        let now = std::time::Instant::now();
        assert!(
            now < deadline,
            "{shell} drew no marked prompt; its output was {:?}",
            String::from_utf8_lossy(&output)
        );
        if now >= next_enter {
            input.send("p1", enter());
            next_enter = now + std::time::Duration::from_millis(500);
        }
        if let Some(Some(proto::stream_message::Message::Output(bytes))) =
            stream.next_within(std::time::Duration::from_millis(100))
        {
            stream.credit(bytes.len() as u64);
            output.extend_from_slice(&bytes);
        }
    }
}

#[test]
fn a_zsh_pane_marks_its_prompts() {
    prompts_are_marked("zsh", true, None);
}

#[test]
fn a_bash_pane_marks_its_prompts() {
    prompts_are_marked("bash", cfg!(target_os = "linux"), None);
}

#[test]
fn a_fish_pane_marks_its_prompts() {
    prompts_are_marked("fish", cfg!(target_os = "linux"), None);
}

/// The shell that ran the command hands the integration to the shell it becomes.
#[test]
fn the_shell_a_command_leaves_behind_marks_its_prompts() {
    for (name, required) in
        [("zsh", true), ("bash", cfg!(target_os = "linux")), ("fish", cfg!(target_os = "linux"))]
    {
        prompts_are_marked(name, required, Some("echo ran"));
    }
}
