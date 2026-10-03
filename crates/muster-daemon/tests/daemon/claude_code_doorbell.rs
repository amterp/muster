//! The flow MIP-4 section 14 moves off `muster pane send`, against the Claude Code installed on
//! this machine: an integrator makes a pane running Claude, posts it a long brief before the
//! session has even shown its prompt, and the agent - started bypassing permission prompts, as
//! workers are, so its inbox would hold the wake - is rung in its pane once its prompt is up and
//! empty, reads the brief whole, and answers with a message that wakes the integrator. A trust
//! dialog in between is left unrung, and answered by the test as a person would.
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
use proto::input_event::{self, Input as Event};
use proto::session_request;

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
    // Everything the daemon logs from before the post on, to see what it typed and when.
    let mut logging = daemon.connect();
    let follow = session_request::Request::FollowLog(session_request::FollowLog { after: None });
    expect(&mut logging, session(follow), proto::Outcome::Done);
    let joined = integrator(&["msg", "join", "--name", "integrator"]);
    assert!(joined.status.success(), "{}", String::from_utf8_lossy(&joined.stderr));
    // Posted before the session has shown its prompt, let alone had its folder trusted: the ring
    // waits for an empty prompt, however long that takes.
    let posted =
        integrator(&["msg", "post", "--to", "worker", "--file", &brief_file.display().to_string()]);
    let said = String::from_utf8_lossy(&posted.stdout);
    assert!(
        posted.status.success(),
        "the post: {said} {}",
        String::from_utf8_lossy(&posted.stderr)
    );
    eprintln!("claude-code: the post said: {said}");

    answer_trust_unrung(&mut control, &mut logging, &mut input, "worker");
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

/// How long a trust dialog is watched for a ring before it is answered: past detection's grace
/// for a new agent and the doorbell's quiet period, both three seconds.
const RINGS_WITHIN: Duration = Duration::from_secs(8);

/// Once Claude Code shows its trust dialog or its prompt: a dialog is watched for a ring for
/// [`RINGS_WITHIN`], and then answered as a person would answer it. `logging` follows the
/// daemon's log from before the post.
fn answer_trust_unrung(
    control: &mut Control,
    logging: &mut Control,
    input: &mut Input,
    pane: &str,
) {
    if !until_prompt_or_trust(control, pane) {
        eprintln!("claude-code: no trust dialog showed, so none was left unrung");
        return;
    }
    let logged = logging.logged_until("msg.rang", RINGS_WITHIN);
    let rang: Vec<&str> = logged
        .iter()
        .map(|line| line.line.as_str())
        .filter(|line| line.contains("\"msg.rang\"") || line.contains("msg.ring.pressed_again"))
        .collect();
    assert!(rang.is_empty(), "the doorbell rang the trust dialog: {rang:?}");
    eprintln!(
        "claude-code: nothing was rung for {}s at the trust dialog; answering it",
        RINGS_WITHIN.as_secs()
    );
    input.send(pane, Event::Send(input_event::Send { text: String::new(), enter: true }));
}

/// Waits until Claude Code shows its trust dialog or its prompt, and says whether it was the
/// dialog.
fn until_prompt_or_trust(control: &mut Control, pane: &str) -> bool {
    until_some(&format!("{pane} to show its trust dialog or its prompt"), || {
        let screen = read_text(control, pane, 0, 0).text;
        if screen.contains("trust") && screen.contains("folder") {
            Some(true)
        } else if screen.contains("? for shortcuts") || screen.contains('❯') {
            Some(false)
        } else {
            None
        }
    })
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

/// The doorbell rings a pane only once the agent's prompt reads empty (MIP-4, section 6), and
/// a newly started Claude Code's prompt is not empty as text: it shows a suggestion,
/// `❯ Try "..."`, that nobody typed. What tells them apart is how it is drawn: faint, with the
/// first letter inverse when Claude Code draws its own caret there (2.1.283 draws none in a
/// pane nothing has focused). This holds that fact to the Claude Code installed.
#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn a_new_claude_prompt_draws_what_nobody_typed_faint() {
    if std::env::var_os("MUSTER_CLAUDE_CODE_TESTS").is_none() {
        eprintln!(
            "claude-code: skipped, MUSTER_CLAUDE_CODE_TESTS is not set; ./dev --claude-code sets it"
        );
        return;
    }
    let arguments = how_to_run().unwrap_or_else(|why| {
        panic!(
            "claude-code: could not run Claude Code: {why}.\n  Impact: nothing checked how a \
             new Claude prompt is drawn, so this tier did not pass.\n  Fix: install claude and \
             log in (`claude auth login`), or set ANTHROPIC_API_KEY."
        )
    });
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
    let project = daemon.root().join("fresh");
    std::fs::create_dir_all(&project).unwrap();
    let command: Vec<String> = arguments.iter().map(|argument| quoted(argument)).collect();
    make(
        &mut control,
        proto::pane_request::Create {
            command: Some(format!("claude {}", command.join(" "))),
            cwd: Some(project.display().to_string()),
            grid: Some(proto::Grid { cols: 110, rows: 35, width_px: 1100, height_px: 700 }),
            ..create("fresh", in_new_tab("t1"))
        },
    );
    until_ready(&mut control, &mut input, "fresh");

    let mut stream = attached(&daemon, "fresh", false);
    let mut surface = Surface::new(110, 35);
    surface.follow(&mut stream, "the prompt", true, |surface| surface.screen().contains('❯'));
    let grid = surface.terminal.viewport(110, 35);
    let row = grid
        .rows
        .iter()
        .find(|row| row.text().trim_start().starts_with('❯'))
        .expect("a prompt row");
    let after: Vec<&muster_vt::Cell> = row
        .cells
        .iter()
        .skip_while(|cell| cell.text != "❯")
        .skip(1)
        .filter(|cell| !cell.text.trim().is_empty())
        .collect();
    let drawn: Vec<(String, bool, bool)> = after
        .iter()
        .map(|cell| (cell.text.clone(), cell.style.inverse, cell.style.faint))
        .collect();
    eprintln!(
        "claude-code: the new prompt reads {:?}; (text, inverse, faint): {drawn:?}",
        row.text()
    );
    let Some((caret, rest)) = after.split_first() else {
        eprintln!("claude-code: the new prompt shows no suggestion, only its caret");
        return;
    };
    assert!(caret.style.inverse || caret.style.faint, "the first cell is typed: {drawn:?}");
    assert!(rest.iter().all(|cell| cell.style.faint), "a cell is not faint: {drawn:?}");
}

/// An urgent post reaches a Claude Code at work (MIP-4, section 6): the doorbell types the wake
/// into its prompt box while a long command runs, and Claude Code queues it and takes it into
/// the running turn once that command returns, as a reminder to address it before going on.
/// Claude Code's own transcript says how it took the line, which is what tells "taken into the
/// turn" from "sent once the turn ended". Whether the model then acts on it before finishing
/// its task is the model's call - Haiku 4.5, which this tier runs, mostly finishes first - so it is
/// printed, not held.
#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn an_urgent_post_reaches_claude_code_mid_turn() {
    use crate::claude_code_council::transcript_folder;
    use crate::claude_code_hooks::{arguments_or_fail, environment, prompt, skipped, start};

    if skipped() {
        return;
    }
    let arguments = arguments_or_fail("that an urgent post reaches Claude Code mid-turn");
    let environment = environment();
    let environment: Vec<(&str, &str)> =
        environment.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let daemon = daemon_with(&environment);
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    let project =
        start(&daemon, &mut control, &mut input, &arguments, "worker", &serde_json::json!({}));
    // Past the grace a newly found agent is held idle through.
    std::thread::sleep(Duration::from_secs(4));
    prompt(
        &mut input,
        "worker",
        "Run this command in the foreground with the Bash tool: for i in 1 2 3 4 5 6 7 8; do \
         date; sleep 5; done. When it finishes, run it once more the same way. Then say DONE.",
    );
    daemon.until_agent("worker", proto::AgentState::Working);
    // Into the command, so the ring is queued behind a tool call that is running.
    std::thread::sleep(Duration::from_secs(10));

    let muster = muster_harness::built_daemon().with_file_name("muster");
    let posted = Command::new(&muster)
        .args(["msg", "post", "--as", "integrator", "--urgent", "--to", "worker"])
        .arg("Read this as soon as you see it; there is nothing to do about it.")
        .env_clear()
        .env("HOME", daemon.root())
        .env("MUSTER_DAEMON_SOCKET", daemon.socket_path())
        .stdin(Stdio::null())
        .output()
        .expect("the muster binary runs");
    let said = String::from_utf8_lossy(&posted.stdout);
    eprintln!("claude-code: the urgent post said: {said}");
    assert!(posted.status.success(), "{said} {}", String::from_utf8_lossy(&posted.stderr));
    assert!(said.contains("woke: worker (working)"), "not rung at work: {said}");

    let home = std::env::var("HOME").expect("HOME is set");
    let folder =
        std::path::Path::new(&home).join(".claude/projects").join(transcript_folder(&project));
    let transcript = || -> Vec<serde_json::Value> {
        let Ok(files) = std::fs::read_dir(&folder) else { return Vec::new() };
        files
            .flatten()
            .filter(|file| file.path().extension().is_some_and(|extension| extension == "jsonl"))
            .flat_map(|file| {
                let text = std::fs::read_to_string(file.path()).unwrap_or_default();
                text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect::<Vec<_>>()
            })
            .collect()
    };
    let taken = |lines: &[serde_json::Value]| -> Vec<String> {
        lines
            .iter()
            .filter(|line| line["type"] == "queue-operation" && line["operation"] == "remove")
            .filter(|line| line["content"].as_str().is_some_and(|text| text.contains("[muster]")))
            .map(|line| line["reason"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    until_turns(Duration::from_mins(3), "Claude Code taking the ring", || {
        !taken(&transcript()).is_empty()
    });
    assert_eq!(
        taken(&transcript()),
        ["absorbed_mid_turn"],
        "Claude Code did not take the ring into the running turn.\n  Impact: an urgent post \
         reaches an agent only once its turn ends, like an ordinary one.\n  Check: how this \
         Claude Code version treats a line typed while it works (docs/observations), and \
         whether its transcript at {} still records queue operations.",
        folder.display()
    );
    until_turns(Duration::from_mins(3), "the worker's turn ending", || {
        snapshot(&mut control)
            .panes
            .iter()
            .any(|pane| pane.pane == "worker" && pane.agent_state() == proto::AgentState::Idle)
    });
    let rewoken = transcript().iter().any(|line| {
        line["type"] == "user"
            && line["message"]["content"].as_str().is_some_and(|text| text.contains("still unread"))
    });
    eprintln!(
        "claude-code: the model read the urgent message {}",
        if rewoken { "only once its turn ended" } else { "before its turn ended" }
    );
}
