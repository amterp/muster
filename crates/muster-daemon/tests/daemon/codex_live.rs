//! The Codex installed on this machine, in panes of a daemon: a pane with `extras/codex`'s hooks
//! reads working and then idle from Codex's own reports and a pane without them reads the same off
//! its screen; Esc mid-turn reads idle from the hooks at once; and a message posted to an idle
//! Codex rings it in its pane, and is read and answered. This is what says whether a Codex update
//! has broken any of it.
//!
//! Out of the gate, because it reaches the network and spends turns of a real model. It runs
//! with the login `codex` already has, or `OPENAI_API_KEY`; with neither, or with no `codex` on
//! the PATH, it fails and says why. `./dev --codex` runs it; any other run of the ignored tests
//! passes it by without trying. Each pane trusts its own scratch folder for the run only, and
//! skips Codex's update check, so nothing is written into `~/.codex/config.toml`
//! (docs/observations/codex-0.154.0.md, section 4).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::claude_code_inbox::{log_of, until_turns};
use crate::claude_code_live::{quoted, until_both_settle};
use crate::support::*;
use muster_harness::Input;
use proto::input_event::{self, Input as Event};

const HOOKS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/codex/hooks/hooks.json"));

const MESSAGING_HOOKS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/codex/messaging-hooks.json"));

/// libghostty's number for the Escape key, as a window sends it.
const KEY_ESCAPE: u32 = 120;

/// Long enough for Codex to start, answer one short prompt and settle.
const TURN: Duration = Duration::from_mins(3);

/// Whether the tier was asked for, and Codex can run here; panics saying why when it cannot.
fn asked_for(checks: &str) -> bool {
    if std::env::var_os("MUSTER_CODEX_TESTS").is_none() {
        eprintln!("codex: skipped, MUSTER_CODEX_TESTS is not set; ./dev --codex sets it");
        return false;
    }
    let version = Command::new("codex").arg("--version").output();
    let Ok(version) = version else {
        panic!(
            "codex: no codex on the PATH.\n  Impact: nothing checked that {checks}, so this \
             tier did not pass.\n  Fix: install Codex (`brew install --cask codex`)."
        )
    };
    eprintln!("codex: {}", String::from_utf8_lossy(&version.stdout).trim());
    let logged_in = std::env::var_os("OPENAI_API_KEY").is_some()
        || Command::new("codex")
            .args(["login", "status"])
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|status| status.status.success());
    assert!(
        logged_in,
        "codex: not logged in and no OPENAI_API_KEY.\n  Impact: nothing checked that {checks}, \
         so this tier did not pass.\n  Fix: `codex login`, or set OPENAI_API_KEY."
    );
    true
}

/// A daemon whose panes can run Codex as this user does.
fn codex_daemon() -> Daemon {
    let home = std::env::var("HOME").expect("HOME is set");
    let mut environment = vec![("HOME", home), ("USER", std::env::var("USER").unwrap_or_default())];
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        environment.push(("OPENAI_API_KEY", key));
    }
    let environment: Vec<(&str, &str)> =
        environment.iter().map(|(name, value)| (*name, value.as_str())).collect();
    daemon_with(&environment)
}

/// A scratch folder for one pane, with `extras/codex`'s hooks as its project hooks when
/// `hooked`, and the command that runs Codex there with `extra` arguments.
fn project(daemon: &Daemon, name: &str, hooked: bool, extra: &[&str]) -> (String, String) {
    project_with(daemon, name, if hooked { &[HOOKS] } else { &[] }, extra)
}

/// [`project`], with each of `hooks` merged into the folder's project hooks.
fn project_with(daemon: &Daemon, name: &str, hooks: &[&str], extra: &[&str]) -> (String, String) {
    let folder = daemon.root().join(name);
    std::fs::create_dir_all(folder.join(".codex")).unwrap();
    // Trust is keyed by the folder Codex resolves, through any symlink: /tmp is /private/tmp.
    let folder = std::fs::canonicalize(folder).unwrap();
    let mut arguments = vec![
        "-c".to_string(),
        "check_for_update_on_startup=false".to_string(),
        "-c".to_string(),
        "notify=[]".to_string(),
        "-c".to_string(),
        "model_reasoning_effort=\"low\"".to_string(),
        "-c".to_string(),
        format!("projects={{\"{}\"={{trust_level=\"trusted\"}}}}", folder.display()),
    ];
    if !hooks.is_empty() {
        std::fs::write(folder.join(".codex/hooks.json"), merged(hooks)).unwrap();
        // Trusting a hook is a person's act in /hooks; a scratch folder's are trusted here.
        arguments.push("--dangerously-bypass-hook-trust".to_string());
    }
    arguments.extend(extra.iter().map(ToString::to_string));
    let quoted_arguments: Vec<String> = arguments.iter().map(|argument| quoted(argument)).collect();
    (folder.display().to_string(), format!("codex {}", quoted_arguments.join(" ")))
}

/// Hooks files as one, each event's groups one after the other, as a person merging them would.
fn merged(files: &[&str]) -> String {
    let mut events = serde_json::Map::new();
    for file in files {
        let hooks: serde_json::Value = serde_json::from_str(file).unwrap();
        for (event, groups) in hooks["hooks"].as_object().unwrap() {
            let into = events.entry(event.clone()).or_insert_with(|| serde_json::json!([]));
            into.as_array_mut().unwrap().extend(groups.as_array().unwrap().iter().cloned());
        }
    }
    serde_json::json!({ "hooks": events }).to_string()
}

/// The `muster` CLI built beside this commit's daemon.
fn built_muster() -> std::path::PathBuf {
    let muster = muster_harness::built_daemon().with_file_name("muster");
    assert!(
        muster.is_file(),
        "no muster CLI at {}.\n  Impact: the agent would have no `muster msg` to run.\n  Fix: \
         run ./dev -b, or cargo build -p muster-cli.",
        muster.display()
    );
    muster
}

/// `command` with `muster` first on its PATH.
fn with_muster_on_path(muster: &std::path::Path, command: &str) -> String {
    let bin = muster.parent().unwrap().display().to_string();
    format!("PATH={}:\"$PATH\" {command}", quoted(&bin))
}

/// Runs `muster` as the integrator, a participant outside any pane.
fn as_integrator(
    muster: &std::path::Path,
    daemon: &Daemon,
    arguments: &[&str],
) -> std::process::Output {
    let ran = Command::new(muster)
        .args(arguments)
        .env_clear()
        .env("HOME", daemon.root())
        .env("MUSTER_DAEMON_SOCKET", daemon.socket_path())
        .stdin(Stdio::null())
        .output()
        .expect("the muster binary runs");
    eprintln!(
        "codex: muster {}: {}{}",
        arguments.first().copied().unwrap_or_default(),
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    ran
}

fn make_pane(control: &mut Control, name: &str, folder: &str, command: String, first: bool) {
    let placement = if first { in_new_tab("t1") } else { beside("hooked", proto::Side::Right) };
    make(
        control,
        proto::pane_request::Create {
            command: Some(command),
            cwd: Some(folder.to_string()),
            grid: Some(proto::Grid { cols: 100, rows: 30, width_px: 1000, height_px: 600 }),
            ..create(name, placement)
        },
    );
}

/// Waits for Codex's composer. Its folder trust question is asked before the composer, which a
/// hooked pane's warning about `--dangerously-bypass-hook-trust` is not.
fn until_ready(control: &mut Control, pane: &str) {
    let deadline = Instant::now() + TURN;
    loop {
        let screen = read_text(control, pane, 0, 0).text;
        let asking = screen.contains("Do you trust the contents of this directory?");
        if screen.lines().any(|line| line.starts_with('›')) && !asking {
            break;
        }
        assert!(Instant::now() < deadline, "{pane}: Codex never showed its composer: {screen}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn type_line(input: &mut Input, pane: &str, text: &str) {
    input.send(pane, Event::Send(input_event::Send { text: text.to_string(), enter: true }));
}

const PANES: [&str; 2] = ["hooked", "screen"];

#[test]
#[ignore = "reaches the network with the real Codex; run through ./dev --codex"]
fn codex_reads_working_then_idle_through_its_hooks_and_through_its_screen() {
    if !asked_for("Codex's hooks and screen still read working and idle") {
        return;
    }
    let daemon = codex_daemon();
    let mut control = daemon.connect();
    control.ask(subscribe_request());
    // In Codex's own sandbox, which refuses a command the daemon's socket: the hooks report
    // from outside it.
    let sandboxed = ["-a", "never", "-s", "workspace-write"];
    for (index, name) in PANES.into_iter().enumerate() {
        let (folder, command) = project(&daemon, name, name == "hooked", &sandboxed);
        make_pane(&mut control, name, &folder, command, index == 0);
    }
    let mut input = Input::connect(daemon.socket_path());
    let mut looking = daemon.connect();
    for name in PANES {
        until_ready(&mut looking, name);
    }
    // Past the grace a newly found agent is held idle through.
    std::thread::sleep(Duration::from_secs(4));
    for name in PANES {
        type_line(
            &mut input,
            name,
            "Without using any tools, write the numbers from 1 to 80, one per line, and nothing else.",
        );
    }
    let settled = until_both_settle(&mut control, PANES);
    for (name, seen) in PANES.into_iter().zip(settled) {
        let summary: Vec<_> =
            seen.iter().map(|record| (record.agent_state(), record.state_reported)).collect();
        eprintln!("codex: {name}: {summary:?}");
        let hooked = name == "hooked";
        // Codex draws its spinner before its first hook has run, so the screen says working a
        // moment before the hooks do.
        let reported_working = seen.iter().any(|record| {
            record.agent_state() == proto::AgentState::Working && record.state_reported
        });
        let idle = seen.iter().rev().find(|record| record.agent_state() == proto::AgentState::Idle);
        assert_eq!(reported_working, hooked, "{name}: working");
        assert_eq!(idle.map(|record| record.state_reported), Some(hooked), "{name}: idle");
        assert_eq!(seen.last().and_then(|record| record.agent.clone()).as_deref(), Some("codex"));
    }
    // Codex has no statusline: its hooks read its context off its transcript, in the background.
    let facts = |control: &mut Control, name: &str| {
        snapshot(control).panes.into_iter().find(|pane| pane.pane == name)?.facts
    };
    let reported = until_turns(Duration::from_secs(20), "the hooked pane's context", || {
        facts(&mut control, "hooked").is_some_and(|facts| facts.context_used.is_some())
    });
    let hooked = facts(&mut control, "hooked");
    eprintln!("codex: hooked facts: {hooked:?}");
    assert!(reported, "the hooked pane's context was never reported: {hooked:?}");
    assert!(hooked.and_then(|facts| facts.model).is_some(), "nor its model");
    assert!(facts(&mut control, "screen").is_none_or(|facts| facts.context_used.is_none()));
}

/// Esc ends a Codex turn with `Interrupt` and no `Stop`; the hooks report that idle, rather than
/// leaving a working report to lapse ten quiet seconds later and the screen to decide.
#[test]
#[ignore = "reaches the network with the real Codex; run through ./dev --codex"]
fn esc_mid_turn_reads_idle_from_codexs_own_report() {
    if !asked_for("Esc mid-turn reads idle from Codex's hooks") {
        return;
    }
    let daemon = codex_daemon();
    let mut control = daemon.connect();
    let (folder, command) =
        project(&daemon, "hooked", true, &["-a", "never", "-s", "workspace-write"]);
    make_pane(&mut control, "hooked", &folder, command, true);
    let mut input = Input::connect(daemon.socket_path());
    until_ready(&mut control, "hooked");
    std::thread::sleep(Duration::from_secs(4));
    type_line(&mut input, "hooked", "Run the shell command sleep 60, and then say done.");
    let reported = |control: &mut Control, state: proto::AgentState| {
        snapshot(control)
            .panes
            .into_iter()
            .find(|pane| pane.pane == "hooked")
            .is_some_and(|pane| pane.agent_state() == state && pane.state_reported)
    };
    assert!(
        until_turns(TURN, "the turn to start", || reported(
            &mut control,
            proto::AgentState::Working
        )),
        "hooked: {}",
        read_text(&mut control, "hooked", 0, 0).text
    );
    std::thread::sleep(Duration::from_secs(5));
    // A key, not text: typed text reaches Codex inside a bracketed paste, where Esc is a byte.
    input.send(
        "hooked",
        Event::Key(input_event::Key {
            action: proto::KeyAction::Press.into(),
            key: KEY_ESCAPE,
            ..input_event::Key::default()
        }),
    );
    let escaped = Instant::now();
    assert!(
        until_turns(Duration::from_secs(20), "Esc to read idle", || {
            reported(&mut control, proto::AgentState::Idle)
        }),
        "hooked: {}",
        read_text(&mut control, "hooked", 0, 0).text
    );
    let took = escaped.elapsed();
    eprintln!("codex: idle by its own report {took:?} after Esc");
    assert!(took < Duration::from_secs(8), "idle came {took:?} after Esc: the report lapsed");
}

/// A message posted to Codex idle in a pane rings it there; it reads the message whole and
/// answers with one of its own. Its sandbox is let reach the network, which is how a sandboxed
/// Codex reaches the daemon (extras/codex/README.md).
#[test]
#[ignore = "reaches the network with the real Codex; run through ./dev --codex"]
fn a_message_posted_to_an_idle_codex_pane_is_rung_read_and_answered() {
    if !asked_for("a Codex pane is rung, reads and answers") {
        return;
    }
    let muster = built_muster();
    let daemon = codex_daemon();
    let mut control = daemon.connect();
    let networked = [
        "-a",
        "never",
        "-s",
        "workspace-write",
        "-c",
        "sandbox_workspace_write.network_access=true",
    ];
    let (folder, command) = project(&daemon, "worker", false, &networked);
    make_pane(&mut control, "worker", &folder, with_muster_on_path(&muster, &command), true);
    until_ready(&mut control, "worker");

    let nonce = format!("c{}", std::process::id());
    let integrator = |arguments: &[&str]| as_integrator(&muster, &daemon, arguments);
    let joined = integrator(&["msg", "join", "--name", "integrator"]);
    assert!(joined.status.success(), "{}", String::from_utf8_lossy(&joined.stderr));
    let body = format!(
        "Answer this message by running exactly: muster msg post --to integrator 'got {nonce}'. \
         Do nothing else."
    );
    let posted = integrator(&["msg", "post", "--to", "worker", &body]);
    let said = String::from_utf8_lossy(&posted.stdout);
    assert!(
        posted.status.success(),
        "the post: {said} {}",
        String::from_utf8_lossy(&posted.stderr)
    );
    eprintln!("codex: the post said: {said}");

    let group = "integrator+worker";
    let answered = until_turns(TURN, "the worker answering", || {
        log_of(&mut control, group).iter().any(|line| line == &format!("worker: got {nonce}"))
    });
    if !answered {
        eprintln!("codex: worker shows:\n{}", read_text(&mut control, "worker", 0, 0).text);
    }
    assert!(answered, "the worker never answered; the log holds {:?}", log_of(&mut control, group));
}

/// An urgent post reaches Codex at work: the ring is typed into its composer without its Return,
/// returned once a second look sees it there alone, and held by Codex for after its running tool
/// call, when the model reads the message and answers it.
#[test]
#[ignore = "reaches the network with the real Codex; run through ./dev --codex"]
fn an_urgent_post_reaches_codex_at_work_and_is_answered() {
    if !asked_for("an urgent post reaches Codex at work") {
        return;
    }
    let muster = built_muster();
    let daemon = codex_daemon();
    let mut control = daemon.connect();
    let networked = [
        "-a",
        "never",
        "-s",
        "workspace-write",
        "-c",
        "sandbox_workspace_write.network_access=true",
    ];
    let (folder, command) = project(&daemon, "worker", true, &networked);
    make_pane(&mut control, "worker", &folder, with_muster_on_path(&muster, &command), true);
    let mut input = Input::connect(daemon.socket_path());
    until_ready(&mut control, "worker");
    std::thread::sleep(Duration::from_secs(4));
    type_line(&mut input, "worker", "Run the shell command sleep 30, and then say done.");
    let working = until_turns(TURN, "the worker to be at work", || {
        snapshot(&mut control)
            .panes
            .into_iter()
            .any(|pane| pane.pane == "worker" && pane.agent_state() == proto::AgentState::Working)
    });
    assert!(working, "worker: {}", read_text(&mut control, "worker", 0, 0).text);
    std::thread::sleep(Duration::from_secs(5));

    let nonce = format!("u{}", std::process::id());
    assert!(
        as_integrator(&muster, &daemon, &["msg", "join", "--name", "integrator"]).status.success()
    );
    let body = format!(
        "Answer this message by running exactly: muster msg post --to integrator 'got {nonce}'."
    );
    let posted =
        as_integrator(&muster, &daemon, &["msg", "post", "--urgent", "--to", "worker", &body]);
    let said = String::from_utf8_lossy(&posted.stdout).to_string();
    assert!(posted.status.success(), "the post: {said}");
    assert!(said.contains("worker (working)"), "rung at work: {said}");

    let group = "integrator+worker";
    let answered = until_turns(TURN, "the worker answering", || {
        log_of(&mut control, group).iter().any(|line| line == &format!("worker: got {nonce}"))
    });
    eprintln!("codex: worker shows:\n{}", read_text(&mut control, "worker", 0, 0).text);
    assert!(answered, "the worker never answered; the log holds {:?}", log_of(&mut control, group));
}

/// With `extras/codex/messaging-hooks.json`, the ring's turn starts with the message already in
/// the model's context, handed over by a hook running outside Codex's sandbox: a Codex whose
/// sandbox may not reach the daemon still reads what it was sent.
#[test]
#[ignore = "reaches the network with the real Codex; run through ./dev --codex"]
fn a_sandboxed_codex_with_messaging_hooks_is_handed_its_message() {
    if !asked_for("Codex's messaging hooks hand it its messages") {
        return;
    }
    let muster = built_muster();
    let daemon = codex_daemon();
    let mut control = daemon.connect();
    let sandboxed = ["-a", "never", "-s", "workspace-write"];
    let (folder, command) = project_with(&daemon, "worker", &[HOOKS, MESSAGING_HOOKS], &sandboxed);
    make_pane(&mut control, "worker", &folder, with_muster_on_path(&muster, &command), true);
    let mut input = Input::connect(daemon.socket_path());
    until_ready(&mut control, "worker");
    std::thread::sleep(Duration::from_secs(4));
    // A first turn starts the session, which is when its hooks first run.
    type_line(&mut input, "worker", "Reply with the single word ready.");
    let settled = until_turns(TURN, "the first turn", || {
        read_text(&mut control, "worker", 0, 0).text.contains("• ready")
    });
    assert!(settled, "worker: {}", read_text(&mut control, "worker", 0, 0).text);
    std::thread::sleep(Duration::from_secs(4));

    let nonce = format!("h{}", std::process::id());
    assert!(
        as_integrator(&muster, &daemon, &["msg", "join", "--name", "integrator"]).status.success()
    );
    let body = format!("The word for today is {nonce}. Reply with it, and do not run any command.");
    let posted = as_integrator(&muster, &daemon, &["msg", "post", "--to", "worker", &body]);
    assert!(posted.status.success());

    let replied = until_turns(TURN, "the worker replying with the word", || {
        read_text(&mut control, "worker", 0, 0)
            .text
            .lines()
            .any(|line| line.starts_with('•') && line.contains(&nonce))
    });
    eprintln!("codex: worker shows:\n{}", read_text(&mut control, "worker", 0, 0).text);
    assert!(replied, "the worker never replied with the word it was sent");
}
