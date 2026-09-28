//! `muster msg` as a person or an agent types it: the real binary against a real daemon built from
//! this commit, reached through `$MUSTER_DAEMON_SOCKET` as a pane would reach it.
//!
//! The child's environment is cleared for the reason `driving_a_window.rs` gives, and more: this
//! suite may run inside a Claude Code session, whose inbox socket in the environment would make
//! every caller that session.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use muster_daemon_proto::messaging;
use muster_harness::Daemon;
use serde_json::Value;

fn muster(daemon: &Daemon, arguments: &[&str]) -> Output {
    muster_with(daemon.socket_path(), arguments, None)
}

fn muster_with(socket: &Path, arguments: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_muster"))
        .args(arguments)
        .env_clear()
        .env("HOME", std::env::temp_dir())
        .env("MUSTER_DAEMON_SOCKET", socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the muster binary runs");
    if let Some(text) = stdin {
        child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    }
    drop(child.stdin.take());
    child.wait_with_output().expect("the muster binary finishes")
}

fn said(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim_end().to_string()
}

fn complained(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim_end().to_string()
}

#[track_caller]
fn ok(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{}", complained(output));
    said(output)
}

#[test]
fn two_agents_post_read_and_are_held_to_the_guard() {
    let daemon = Daemon::start_built();
    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "a", "join", "--group", "review"])),
        "created review and joined it as a"
    );
    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "b", "join", "--group", "review"])),
        "joined review as b"
    );

    let posted = ok(&muster(&daemon, &["msg", "--as", "a", "post", "the", "parser", "is", "in"]));
    assert_eq!(posted, "posted #4 to review\nnot woken: b (sees it when it reads)");

    let refused = muster(&daemon, &["msg", "--as", "b", "post", "done"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        complained(&refused).contains("muster msg read --group review"),
        "the refusal names the read that clears it: {}",
        complained(&refused)
    );

    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "b", "read"])),
        "--- review #4 | a ---\nthe parser is in\n--- end review #4 | a ---"
    );
    assert_eq!(ok(&muster(&daemon, &["msg", "--as", "b", "read"])), "nothing unread");
    assert_eq!(ok(&muster(&daemon, &["msg", "--as", "b", "read", "--if-unread"])), "");
    ok(&muster(&daemon, &["msg", "--as", "b", "post", "--to", "a", "done"]));

    let log = ok(&muster(&daemon, &["msg", "--json", "log", "--group", "review"]));
    let log: Value = serde_json::from_str(&log).unwrap();
    let bodies: Vec<&str> = log["groups"][0]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["body"].as_str())
        .collect();
    assert_eq!(bodies, ["the parser is in", "done"]);
}

#[test]
fn a_body_comes_whole_from_a_file_or_stdin() {
    let daemon = Daemon::start_built();
    ok(&muster(&daemon, &["msg", "--as", "a", "join", "--group", "g"]));
    ok(&muster(&daemon, &["msg", "--as", "b", "join", "--group", "g"]));
    // Longer than Claude Code folds a paste at, with the lines a brief has.
    let brief = "a line of the brief\n".repeat(400);
    let file = daemon.root().join("brief.md");
    std::fs::write(&file, &brief).unwrap();

    ok(&muster(&daemon, &["msg", "--as", "a", "post", "--file", &file.display().to_string()]));
    ok(&muster_with(daemon.socket_path(), &["msg", "--as", "a", "post", "-"], Some("from stdin")));

    let read = ok(&muster(&daemon, &["msg", "--json", "--as", "b", "read"]));
    let read: Value = serde_json::from_str(&read).unwrap();
    let bodies: Vec<&str> = read["groups"][0]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["body"].as_str())
        .collect();
    assert_eq!(bodies, [brief.as_str(), "from stdin"]);
}

#[test]
fn a_wait_prints_the_wake_and_a_timeout_exits_5() {
    let daemon = Daemon::start_built();
    ok(&muster(&daemon, &["msg", "--as", "a", "join", "--group", "g"]));
    ok(&muster(&daemon, &["msg", "--as", "b", "join", "--group", "g"]));

    let timed_out = muster(&daemon, &["msg", "--as", "b", "wait", "--timeout", "1"]);
    assert_eq!(timed_out.status.code(), Some(5), "{}", complained(&timed_out));

    ok(&muster(&daemon, &["msg", "--as", "a", "post", "go"]));
    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "b", "wait"])),
        "[muster] g: 1 new (#4), from a. Read: muster msg read --group g"
    );
}

#[test]
fn no_daemon_to_ask_exits_3() {
    let nowhere =
        std::env::temp_dir().join(format!("muster-no-daemon-{}.sock", std::process::id()));
    let ran = muster_with(&nowhere, &["msg", "who"], None);
    assert_eq!(ran.status.code(), Some(3), "{}", complained(&ran));
    assert!(complained(&ran).contains("no muster-daemon answered"), "{}", complained(&ran));
}

#[test]
fn a_post_needs_a_message() {
    let daemon = Daemon::start_built();
    let ran = muster(&daemon, &["msg", "post"]);
    assert_eq!(ran.status.code(), Some(1));
    assert!(complained(&ran).contains("needs a message"), "{}", complained(&ran));
}

/// The verbs are spelled in one place, and the reference has to spell them the same way.
#[test]
fn the_reference_spells_every_verb_as_the_command_does() {
    let reference = include_str!("../../../../docs/cli/msg.md");
    for verb in messaging::VERBS {
        let spelled = messaging::command(verb, "");
        let in_table = format!("| `{verb}");
        assert!(
            reference.contains(&spelled) || reference.contains(&in_table),
            "docs/cli/msg.md never spells `{spelled}`"
        );
    }
    let help = muster_with(Path::new("/nonexistent"), &["msg", "--help"], None);
    let help = said(&help);
    for verb in messaging::VERBS {
        assert!(help.contains(verb), "`muster msg --help` does not list {verb}: {help}");
    }
}
