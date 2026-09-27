//! A pane's `ssh` and `sudo` against the devenv container: a real ssh to a Debian host that has
//! no xterm-ghostty entry, and a real sudo there. `ssh_terminfo.rs` runs the same ssh path
//! against a stand-in; this is what says a real sshd, tic and sudo agree with it.
//!
//! Out of the default gate like the other devenv tests: `./dev --ssh` runs it.

mod support;

use std::io::Write;
use std::process::{Command, Stdio};

use muster_harness::Input;
use support::*;

/// The host and the ssh options `./dev --ssh` hands every devenv test.
fn devenv() -> (String, Vec<String>) {
    let host = std::env::var("MUSTER_DEVENV_HOST").expect(
        "MUSTER_DEVENV_HOST is unset, so this test has no machine to talk to. Run it through \
         ./dev --ssh, which starts the container and sets it.",
    );
    let options = std::env::var("MUSTER_DEVENV_SSH_OPTIONS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    (host, options)
}

/// Runs `script` on the host, from this process rather than from a pane, feeding it `stdin`.
fn on_host(script: &str, stdin: &[u8]) -> std::process::Output {
    let (host, options) = devenv();
    let mut child = Command::new("ssh")
        .args(&options)
        .args(["-o", "LogLevel=ERROR", &host, script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("ssh runs");
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

/// The first ssh from a pane gives the host the entry, and the host is remembered: after the
/// entry is taken away again, the next ssh still says xterm-ghostty and installs nothing.
#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn ssh_from_a_pane_installs_the_entry_on_a_host_once() {
    let (host, options) = devenv();
    let check = on_host("rm -rf ~/.terminfo; infocmp xterm-ghostty", b"");
    assert!(
        !check.status.success(),
        "the host already has xterm-ghostty without ~/.terminfo, so this test proves nothing"
    );

    let daemon = daemon();
    std::fs::write(daemon.root().join("home/.zshrc"), "PS1='local> '\n").unwrap();
    let mut control = daemon.connect();
    let zsh = proto::Shell {
        command: Some("zsh".to_string()),
        mode: proto::ShellMode::Login.into(),
        ..proto::Shell::default()
    };
    expect(
        &mut control,
        session(proto::session_request::Request::SetShell(proto::SetShell { shell: Some(zsh) })),
        proto::Outcome::Done,
    );
    make(
        &mut control,
        proto::pane_request::Create {
            grid: Some(proto::Grid { cols: 200, rows: 50, width_px: 2000, height_px: 1000 }),
            ..create("p1", in_new_tab("t1"))
        },
    );
    let mut input = Input::connect(daemon.socket_path());
    let mut typed = |line: &str| {
        let send = proto::input_event::Send { text: line.to_string(), enter: true };
        input.send("p1", proto::input_event::Input::Send(send));
    };
    // Counting each shell's prompts says which one the next line reaches. bash's on the host is
    // Debian's, "dev@<container>:~$".
    let (local, remote) = ("local>", ":~$");
    shown(&mut control, local, 1);

    let ssh = format!("ssh {} -o LogLevel=ERROR {host}", options.join(" "));
    // Linux, so a line that reached the local shell instead cannot pass for the host's answer.
    let report = "[ \"$(uname -s)\" = Linux ] && echo \"host-$TERM-$TERM_PROGRAM-$(infocmp \
                  xterm-ghostty >/dev/null 2>&1 && echo has || echo lacks)\"; exit";

    typed(&ssh);
    shown(&mut control, remote, 1);
    typed(report);
    shown(&mut control, "host-xterm-ghostty-ghostty-has", 1);
    shown(&mut control, local, 2);

    let cache = daemon.root().join("home/.muster/state/ssh-terminfo");
    assert_eq!(std::fs::read_to_string(&cache).unwrap(), "dev@localhost\n");

    assert!(on_host("rm -rf ~/.terminfo", b"").status.success());
    typed(&ssh);
    shown(&mut control, remote, 2);
    typed(report);
    let text = shown(&mut control, "host-xterm-ghostty-ghostty-lacks", 1);
    let installs = text.matches("Setting up xterm-ghostty terminfo on dev@localhost").count();
    assert_eq!(installs, 1, "{text}");
}

/// Waits for the pane to show `wanted` at least `times` times, and returns what it shows.
fn shown(control: &mut Control, wanted: &str, times: usize) -> String {
    until_some(&format!("the pane to show {wanted:?} {times} time(s)"), || {
        let text = read_text(control, "p1", 0, 0).text;
        (text.matches(wanted).count() >= times).then_some(text)
    })
}

/// Ghostty's `sudo` wrapper, from the daemon's data, carries the daemon's `$TERMINFO` through a
/// real sudo to a root that would otherwise not find xterm-ghostty. The data's terminfo and the
/// bash integration are copied to the host as they are, and `$TERMINFO` names the copy, as the
/// daemon names its own.
#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn sudo_through_the_wrapper_finds_the_daemons_entry() {
    let tar = Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .args(["-C", DAEMON_DATA, "-cf", "-", "terminfo", "shell-integration/bash/ghostty.bash"])
        .output()
        .unwrap();
    assert!(tar.status.success(), "{}", String::from_utf8_lossy(&tar.stderr));
    let unpacked = on_host(
        "rm -rf /tmp/muster-sudo && mkdir -p /tmp/muster-sudo && tar -xf - -C /tmp/muster-sudo \
         2>/dev/null",
        &tar.stdout,
    );
    assert!(unpacked.status.success(), "{}", String::from_utf8_lossy(&unpacked.stderr));

    let ran = on_host(
        "GHOSTTY_SHELL_FEATURES=sudo TERMINFO=/tmp/muster-sudo/terminfo bash --norc -i -c '\
         . /tmp/muster-sudo/shell-integration/bash/ghostty.bash
         sudo -n infocmp xterm-ghostty >/dev/null && echo wrapped-finds
         command sudo -n infocmp xterm-ghostty >/dev/null 2>&1 || echo bare-does-not' 2>&1",
        b"",
    );
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(said.contains("wrapped-finds"), "sudo through the wrapper lost TERMINFO: {said}");
    assert!(said.contains("bare-does-not"), "root finds the entry without it: {said}");
}
