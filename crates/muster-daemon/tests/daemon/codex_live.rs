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

const HOOKS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/codex/hooks/hooks.json");

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
    if hooked {
        std::fs::copy(HOOKS, folder.join(".codex/hooks.json")).unwrap();
        // Trusting a hook is a person's act in /hooks; a scratch folder's are trusted here.
        arguments.push("--dangerously-bypass-hook-trust".to_string());
    }
    arguments.extend(extra.iter().map(ToString::to_string));
    let quoted_arguments: Vec<String> = arguments.iter().map(|argument| quoted(argument)).collect();
    (folder.display().to_string(), format!("codex {}", quoted_arguments.join(" ")))
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

/// Waits for Codex's composer.
fn until_ready(control: &mut Control, pane: &str) {
    let deadline = Instant::now() + TURN;
    loop {
        let screen = read_text(control, pane, 0, 0).text;
        if screen.lines().any(|line| line.starts_with('›')) && !screen.contains("trust") {
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
            "Write the numbers from 1 to 80, one per line, and nothing else.",
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
    let muster = muster_harness::built_daemon().with_file_name("muster");
    assert!(
        muster.is_file(),
        "no muster CLI at {}.\n  Impact: the agent would have no `muster msg` to run.\n  Fix: \
         run ./dev -b, or cargo build -p muster-cli.",
        muster.display()
    );
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
    let bin = muster.parent().unwrap().display().to_string();
    let command = format!("PATH={}:\"$PATH\" {command}", quoted(&bin));
    make_pane(&mut control, "worker", &folder, command, true);
    until_ready(&mut control, "worker");

    let nonce = format!("c{}", std::process::id());
    let integrator = |arguments: &[&str]| {
        Command::new(&muster)
            .args(arguments)
            .env_clear()
            .env("HOME", daemon.root())
            .env("MUSTER_DAEMON_SOCKET", daemon.socket_path())
            .stdin(Stdio::null())
            .output()
            .expect("the muster binary runs")
    };
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
