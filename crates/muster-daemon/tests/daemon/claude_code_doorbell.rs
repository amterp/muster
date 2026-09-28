//! The flow MIP-4 section 14 moves off `muster pane send`, against the Claude Code installed on
//! this machine: an integrator makes a pane running Claude, posts it a long brief before the
//! session has even shown its prompt, and the agent - started bypassing permission prompts, as
//! workers are, so its inbox would hold the wake - is rung in its pane once it is idle, reads
//! the brief whole, and answers with a message that wakes the integrator.
//!
//! Ignored by the gate, which may not reach the network; `./dev --claude-code` runs it.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::claude_code_inbox::{log_of, until_turns};
use crate::claude_code_live::{how_to_run, quoted, until_ready};
use crate::support::*;
use muster_harness::Input;

/// About the size of a brief the integrator writes: past the length at which Claude Code folds
/// a paste into a placeholder, and under what its Bash tool shows of a command's output.
const BRIEF_BYTES: usize = 20 * 1024;

#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn a_brief_posted_to_a_new_claude_pane_is_rung_read_whole_and_answered() {
    if std::env::var_os("MUSTER_CLAUDE_CODE_TESTS").is_none() {
        eprintln!(
            "claude-code: skipped, MUSTER_CLAUDE_CODE_TESTS is not set; ./dev --claude-code sets it"
        );
        return;
    }
    let arguments = how_to_run().unwrap_or_else(|why| {
        panic!(
            "claude-code: could not run Claude Code: {why}.\n  Impact: nothing checked that an \
             agent in a pane is reached by a posted brief, so this tier did not pass.\n  Fix: \
             install claude and log in (`claude auth login`), or set ANTHROPIC_API_KEY."
        )
    });
    let muster = muster_harness::built_daemon().with_file_name("muster");
    assert!(
        muster.is_file(),
        "no muster CLI at {}.\n  Impact: the agent would have no `muster msg` to run.\n  Fix: \
         run ./dev -b, or cargo build -p muster-cli.",
        muster.display()
    );
    let bin = muster.parent().unwrap().display().to_string();

    let home = std::env::var("HOME").expect("HOME is set");
    let mut environment = vec![("HOME", home), ("USER", std::env::var("USER").unwrap_or_default())];
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        environment.push(("ANTHROPIC_API_KEY", key));
    }
    let environment: Vec<(&str, &str)> =
        environment.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let daemon = daemon_with(&environment);
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());

    // The worker, as the integrator starts one: bypassing permission prompts, with nothing in
    // its settings about messages from other sessions.
    let project = daemon.root().join("worker");
    std::fs::create_dir_all(&project).unwrap();
    let settings = project.join("settings.json");
    std::fs::write(&settings, r#"{"skipDangerousModePermissionPrompt":true}"#).unwrap();
    let mut command: Vec<String> = arguments.iter().map(|argument| quoted(argument)).collect();
    command.push(format!("--settings {}", quoted(&settings.display().to_string())));
    command.push("--permission-mode bypassPermissions".to_string());
    make(
        &mut control,
        proto::pane_request::Create {
            command: Some(format!("PATH={}:\"$PATH\" claude {}", quoted(&bin), command.join(" "))),
            cwd: Some(project.display().to_string()),
            grid: Some(proto::Grid { cols: 110, rows: 35, width_px: 1100, height_px: 700 }),
            ..create("worker", in_new_tab("t1"))
        },
    );

    // The integrator: a session whose inbox is a socket here, so the answer's wake is seen.
    let inbox = daemon.root().join("integrator.inbox.sock");
    let listener = UnixListener::bind(&inbox).unwrap();
    listener.set_nonblocking(true).unwrap();
    let nonce = format!("w{}", std::process::id());
    let brief = brief(&nonce);
    assert!(brief.len() >= BRIEF_BYTES, "the brief is {} bytes", brief.len());
    let brief_file = daemon.root().join("brief.md");
    std::fs::write(&brief_file, &brief).unwrap();
    let integrator = |arguments: &[&str]| {
        Command::new(&muster)
            .args(arguments)
            .env_clear()
            .env("HOME", daemon.root())
            .env("MUSTER_DAEMON_SOCKET", daemon.socket_path())
            .env("CLAUDE_CODE_MESSAGING_SOCKET", &inbox)
            .stdin(Stdio::null())
            .output()
            .expect("the muster binary runs")
    };
    let joined = integrator(&["msg", "join", "--name", "integrator"]);
    assert!(joined.status.success(), "{}", String::from_utf8_lossy(&joined.stderr));
    // Posted before the session has shown its prompt, let alone had its folder trusted: the ring
    // waits for an idle agent, however long that takes.
    let posted =
        integrator(&["msg", "post", "--to", "worker", "--file", &brief_file.display().to_string()]);
    let said = String::from_utf8_lossy(&posted.stdout);
    assert!(
        posted.status.success(),
        "the post: {said} {}",
        String::from_utf8_lossy(&posted.stderr)
    );
    eprintln!("claude-code: the post said: {said}");

    until_ready(&mut control, &mut input, "worker");
    let group = "integrator+worker";
    let answered = until_turns(Duration::from_mins(5), "the worker answering", || {
        log_of(&mut control, group).iter().any(|line| line == &format!("worker: got {nonce}"))
    });
    if !answered {
        eprintln!("claude-code: worker shows:\n{}", read_text(&mut control, "worker", 0, 0).text);
    }
    assert!(answered, "the worker never answered; the log holds {:?}", log_of(&mut control, group));

    // The daemon also connects to see whether the inbox is there, sending nothing; the wake is
    // the connection with a line on it.
    let told = until_some("the integrator's inbox to be sent a wake", || {
        let (connection, _) = listener.accept().ok()?;
        connection.set_nonblocking(false).ok()?;
        BufReader::new(connection).lines().next()?.ok()
    });
    assert!(told.contains(&format!("[muster] {group}: 1 new")), "the integrator was told {told}");
}

/// A brief as long as an integrator's, whose last line holds the word the answer must carry.
fn brief(nonce: &str) -> String {
    let mut brief = String::from(
        "You are a worker. This brief is long, and its last line holds a word. Read all of \
         it, then run `muster msg post --to integrator \"got WORD\"` with WORD replaced by that \
         word, and end your turn. Do nothing else.\n\n",
    );
    let mut line = 0;
    while brief.len() < BRIEF_BYTES {
        line += 1;
        let _ = writeln!(
            brief,
            "Context {line}: this line stands for the background a real brief carries; there \
             is nothing to do with it."
        );
    }
    let _ = write!(brief, "\nThe word is {nonce}.\n");
    brief
}
