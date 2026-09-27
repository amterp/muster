//! What a pane's programs are told about the terminal they run in (MIP-3 section 3).

use crate::support::*;

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

/// Each pane's shell integration is told what the app's cursor setting was when the pane started.
#[test]
fn the_prompt_cursor_follows_the_cursor_the_app_sent() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let features = |control: &mut Control, pane: &str, tab: &str| {
        let out = daemon.root().join(pane);
        let mut asked = create(pane, in_new_tab(tab));
        asked.command = Some(format!("echo \"$GHOSTTY_SHELL_FEATURES\" > {}", out.display()));
        make(control, asked);
        until_some("the pane to write its features", || {
            std::fs::read_to_string(&out).ok().filter(|text| text.ends_with('\n'))
        })
    };
    assert_eq!(features(&mut control, "p1", "t1"), "cursor:blink,ssh-env,ssh-terminfo,title\n");

    let steady =
        proto::Cursor { style: proto::CursorStyle::Unspecified.into(), blink: Some(false) };
    let set = proto::SetCursor { cursor: Some(steady) };
    expect(
        &mut control,
        session(proto::session_request::Request::SetCursor(set)),
        proto::Outcome::Done,
    );
    assert_eq!(features(&mut control, "p2", "t2"), "cursor:steady,ssh-env,ssh-terminfo,title\n");

    let bar = proto::Cursor { style: proto::CursorStyle::Bar.into(), blink: None };
    let set = proto::SetCursor { cursor: Some(bar) };
    expect(
        &mut control,
        session(proto::session_request::Request::SetCursor(set)),
        proto::Outcome::Done,
    );
    assert_eq!(features(&mut control, "p3", "t3"), "ssh-env,ssh-terminfo,title\n");
}

#[test]
fn a_daemon_without_its_data_directory_refuses_to_start() {
    let scratch = std::env::temp_dir().join(format!("muster-no-data-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
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
    // The refusal is in the daemon's own log beside the socket, so the directory is not empty.
    std::fs::remove_dir_all(&scratch).unwrap();
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
    shell_prompts_are_marked(&daemon(), &shell, command);
}

/// [`prompts_are_marked`] for the shell at `shell`, on `daemon`.
fn shell_prompts_are_marked(daemon: &Daemon, shell: &str, command: Option<&str>) {
    let shell = shell.to_string();
    // zsh with no startup file of its own offers to write one instead of prompting.
    std::fs::write(daemon.root().join("home/.zshrc"), "").unwrap();
    let mut control = daemon.connect();
    let set = proto::SetShell {
        shell: Some(proto::Shell {
            command: Some(shell.clone()),
            mode: proto::ShellMode::Login.into(),
            ..proto::Shell::default()
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
    let mut stream = attached(daemon, "p1", false);
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

/// The exec line runs after the command, in whatever the command left behind.
#[test]
fn a_command_that_empties_path_still_leaves_a_marked_prompt() {
    for (name, required) in
        [("zsh", true), ("bash", cfg!(target_os = "linux")), ("fish", cfg!(target_os = "linux"))]
    {
        prompts_are_marked(name, required, Some("PATH=/nonexistent"));
    }
}

/// `env` would read a path containing `=` as a variable to set.
#[test]
fn a_shell_whose_path_has_an_equals_sign_runs_a_command_and_becomes_itself() {
    let zsh = installed("zsh").expect("zsh is expected on every machine this suite runs on");
    let daemon = daemon();
    let dir = daemon.root().join("a=b");
    std::fs::create_dir(&dir).unwrap();
    let linked = dir.join("zsh");
    std::os::unix::fs::symlink(zsh, &linked).unwrap();
    shell_prompts_are_marked(&daemon, &linked.display().to_string(), Some("echo ran"));
}

/// Starts `shell` with its integration and types `line` until the pane shows `wanted`: the
/// integration defines its functions only once the shell has drawn a prompt.
fn shell_says(shell: &str, sudo: Option<bool>, line: &str, wanted: &str) -> (Daemon, String) {
    let daemon = daemon();
    std::fs::write(daemon.root().join("home/.zshrc"), "").unwrap();
    let mut control = daemon.connect();
    let set = proto::SetShell {
        shell: Some(proto::Shell {
            command: Some(shell.to_string()),
            mode: proto::ShellMode::Login.into(),
            sudo,
            ..proto::Shell::default()
        }),
    };
    expect(
        &mut control,
        session(proto::session_request::Request::SetShell(set)),
        proto::Outcome::Done,
    );
    make(&mut control, create("p1", in_new_tab("t1")));
    let mut input = muster_harness::Input::connect(daemon.socket_path());
    let text = until_some(&format!("{shell} to say {wanted:?}"), || {
        let send = proto::input_event::Send { text: line.to_string(), enter: true };
        input.send("p1", proto::input_event::Input::Send(send));
        std::thread::sleep(std::time::Duration::from_millis(300));
        let text = read_text(&mut control, "p1", 0, 0).text;
        text.contains(wanted).then_some(text)
    });
    drop(control);
    (daemon, text)
}

/// Ghostty's `ssh-*` features wrap ssh to give the host the entry (`tests/ssh_terminfo.rs`), and
/// are on unless the settings turn them off. Its `sudo` feature, which wraps sudo to keep
/// `$TERMINFO`, is off unless turned on: preserving `TERMINFO` needs a sudoers rule that allows
/// SETENV, and sudo refuses outright under one that does not. So by default sudo is the
/// system's, and nothing sets `$TERMINFO`.
#[test]
fn by_default_ssh_is_wrapped_and_sudo_is_left_alone() {
    for shell in ["zsh", "bash"] {
        let Some(path) = installed(shell) else {
            eprintln!("skipped: {shell} with integration is not installed here");
            continue;
        };
        let line = "type sudo ssh; echo \"terminfo=[$TERMINFO]\"";
        let (_daemon, text) = shell_says(&path, None, line, "\nterminfo=[]");
        assert!(!text.contains("sudo is a shell function"), "{shell}: sudo is wrapped: {text}");
        assert!(!text.contains("sudo is a function"), "{shell}: sudo is wrapped: {text}");
        assert!(text.contains("ssh is a"), "{shell}: ssh is not wrapped: {text}");
    }
}

/// With `sudo` on, sudo is wrapped and `$TERMINFO` is the pane's `~/.terminfo`, where the daemon
/// has put the entry: what root reads through the wrapper, and where tic writes, rather than the
/// daemon's data directory, which can be a signed bundle.
#[test]
fn with_sudo_on_sudo_carries_the_home_terminfo_that_holds_the_entry() {
    for shell in ["zsh", "bash"] {
        let Some(path) = installed(shell) else {
            eprintln!("skipped: {shell} with integration is not installed here");
            continue;
        };
        let line = "type sudo; echo \"terminfo=[$TERMINFO]\"";
        let (daemon, text) = shell_says(&path, Some(true), line, "/.terminfo]");
        assert!(text.contains("sudo is a"), "{shell}: sudo is not wrapped: {text}");
        let home = daemon.root().join("home/.terminfo");
        assert!(text.contains(&format!("terminfo=[{}]", home.display())), "{shell}: {text}");
        let entries = [home.join("78/xterm-ghostty"), home.join("x/xterm-ghostty")];
        assert!(entries.iter().any(|entry| entry.exists()), "{shell}: no entry in {home:?}");
    }
}
