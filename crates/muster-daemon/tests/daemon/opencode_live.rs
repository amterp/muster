//! OpenCode, in panes of a daemon: a pane with `extras/opencode`'s plugin reads working and then
//! idle from OpenCode's own reports, and a pane without it reads the same off its screen; and a
//! message posted to an idle OpenCode is rung in its pane, and is read and answered. This is what
//! says whether an OpenCode update has broken any of it.
//!
//! Out of the gate, because it reaches the network and spends turns of a real model - a free one,
//! `opencode/big-pickle`, which OpenCode's own provider serves with no key. Each pane's data,
//! config and caches are in the test's scratch folder, and the daemon's environment holds no
//! credential, so no stored login is reachable. OpenCode's free models refuse versions before
//! 1.18 (docs/observations/opencode-1.18.34.md), so the tier needs 1.18 or later:
//! `MUSTER_OPENCODE_BINARY` names one, else `opencode` on the PATH is used. `./dev --opencode`
//! runs it; any other run of the ignored tests passes it by without trying.

use std::process::Command;
use std::time::{Duration, Instant};

use crate::claude_code_inbox::{log_of, until_turns};
use crate::claude_code_live::{quoted, until_both_settle};
use crate::codex_live::{as_integrator, built_muster, type_line, with_muster_on_path};
use crate::support::*;
use muster_harness::Input;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/opencode/plugin/muster.js");

/// The free model every pane runs.
const MODEL: &str = "opencode/big-pickle";

/// The first OpenCode whose free models answer it.
const SINCE: (u32, u32) = (1, 18);

/// Long enough for OpenCode to start, answer one short prompt and settle.
const TURN: Duration = Duration::from_mins(3);

const PANES: [&str; 2] = ["hooked", "screen"];

/// The OpenCode to run, when the tier was asked for and it can run here; panics saying what is
/// missing when it cannot.
fn asked_for(checks: &str) -> Option<String> {
    if std::env::var_os("MUSTER_OPENCODE_TESTS").is_none() {
        eprintln!("opencode: skipped, MUSTER_OPENCODE_TESTS is not set; ./dev --opencode sets it");
        return None;
    }
    let binary = std::env::var("MUSTER_OPENCODE_BINARY").unwrap_or_else(|_| "opencode".into());
    let said = Command::new(&binary).arg("--version").output().unwrap_or_else(|error| {
        panic!(
            "opencode: {binary} could not be run: {error}.\n  Impact: nothing checked that \
             {checks}, so this tier did not pass.\n  Fix: install OpenCode 1.18 or later, or \
             point MUSTER_OPENCODE_BINARY at one."
        )
    });
    let version = String::from_utf8_lossy(&said.stdout).trim().to_string();
    let mut parts = version.split('.').map(|part| part.parse::<u32>().unwrap_or(0));
    let found = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    assert!(
        found >= SINCE,
        "opencode: {binary} is {version}, and OpenCode's free models refuse anything before \
         {}.{}.\n  Impact: nothing checked that {checks}, so this tier did not pass.\n  Fix: \
         install a current OpenCode (npm install --prefix <folder> opencode-ai) and point \
         MUSTER_OPENCODE_BINARY at <folder>/node_modules/.bin/opencode.",
        SINCE.0,
        SINCE.1
    );
    eprintln!("opencode: {binary} {version}");
    Some(binary)
}

/// A scratch folder for one pane, with OpenCode's data and config in it, the plugin installed
/// when `hooked`, and the command that runs OpenCode there.
fn project(daemon: &Daemon, binary: &str, name: &str, hooked: bool) -> (String, String) {
    let folder = daemon.root().join(name);
    let home = folder.join(".opencode-home");
    let mut variables = Vec::new();
    for variable in ["XDG_DATA_HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"] {
        let path = home.join(variable.to_lowercase());
        std::fs::create_dir_all(&path).unwrap();
        variables.push(format!("{variable}={}", quoted(&path.display().to_string())));
    }
    let config = home.join("xdg_config_home/opencode");
    std::fs::create_dir_all(config.join("plugin")).unwrap();
    // A ring is read with `muster msg read`, which a prompt to allow it would hold up.
    let settings = serde_json::json!({ "autoupdate": false, "permission": { "bash": "allow" } });
    std::fs::write(config.join("opencode.json"), settings.to_string()).unwrap();
    if hooked {
        std::fs::copy(PLUGIN, config.join("plugin/muster.js")).unwrap();
    }
    let folder = std::fs::canonicalize(folder).unwrap();
    let command = format!("{} {} -m {MODEL}", variables.join(" "), quoted(binary));
    (folder.display().to_string(), command)
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

/// Waits for OpenCode's prompt box.
fn until_ready(control: &mut Control, pane: &str) {
    let deadline = Instant::now() + TURN;
    loop {
        let screen = read_text(control, pane, 0, 0).text;
        if screen.contains("Ask anything") {
            break;
        }
        assert!(Instant::now() < deadline, "{pane}: OpenCode never showed its prompt: {screen}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "reaches the network with a real OpenCode; run through ./dev --opencode"]
fn opencode_reads_working_then_idle_through_its_plugin_and_through_its_screen() {
    let Some(binary) = asked_for("OpenCode's plugin and screen still read working and idle") else {
        return;
    };
    let daemon = daemon_with(&[]);
    let mut control = daemon.connect();
    control.ask(subscribe_request());
    for (index, name) in PANES.into_iter().enumerate() {
        let (folder, command) = project(&daemon, &binary, name, name == "hooked");
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
        eprintln!("opencode: {name}: {summary:?}");
        let hooked = name == "hooked";
        let reported_working = seen.iter().any(|record| {
            record.agent_state() == proto::AgentState::Working && record.state_reported
        });
        let idle = seen.iter().rev().find(|record| record.agent_state() == proto::AgentState::Idle);
        assert_eq!(reported_working, hooked, "{name}: working");
        assert_eq!(idle.map(|record| record.state_reported), Some(hooked), "{name}: idle");
        assert_eq!(
            seen.last().and_then(|record| record.agent.clone()).as_deref(),
            Some("opencode")
        );
    }
    let facts = |control: &mut Control, name: &str| {
        snapshot(control).panes.into_iter().find(|pane| pane.pane == name)?.facts
    };
    let reported = until_turns(Duration::from_secs(20), "the hooked pane's context", || {
        facts(&mut control, "hooked").is_some_and(|facts| facts.context_used.is_some())
    });
    let hooked = facts(&mut control, "hooked");
    eprintln!("opencode: hooked facts: {hooked:?}");
    assert!(reported, "the hooked pane's context was never reported: {hooked:?}");
    assert_eq!(hooked.and_then(|facts| facts.model).as_deref(), Some(MODEL));
    assert!(facts(&mut control, "screen").is_none_or(|facts| facts.context_used.is_none()));
}

/// A message posted to an idle OpenCode is rung at its empty prompt box, one paste and a Return,
/// which OpenCode 1.18 sends; the model reads the message with `muster msg read` and answers it.
#[test]
#[ignore = "reaches the network with a real OpenCode; run through ./dev --opencode"]
fn a_message_posted_to_an_idle_opencode_pane_is_rung_read_and_answered() {
    let Some(binary) = asked_for("an OpenCode pane is rung, reads and answers") else {
        return;
    };
    let muster = built_muster();
    let daemon = daemon_with(&[]);
    let mut control = daemon.connect();
    let (folder, command) = project(&daemon, &binary, "worker", false);
    make_pane(&mut control, "worker", &folder, with_muster_on_path(&muster, &command), true);
    until_ready(&mut control, "worker");

    let nonce = format!("o{}", std::process::id());
    let integrator = |arguments: &[&str]| as_integrator(&muster, &daemon, arguments);
    assert!(integrator(&["msg", "join", "--name", "integrator"]).status.success());
    let body = format!(
        "Answer this message by running exactly: muster msg post --to integrator 'got {nonce}'. \
         Do nothing else."
    );
    let posted = integrator(&["msg", "post", "--to", "worker", &body]);
    assert!(posted.status.success(), "{}", String::from_utf8_lossy(&posted.stderr));

    let group = "integrator+worker";
    let answered = until_turns(TURN, "the worker answering", || {
        log_of(&mut control, group).iter().any(|line| line == &format!("worker: got {nonce}"))
    });
    if !answered {
        eprintln!("opencode: worker shows:\n{}", read_text(&mut control, "worker", 0, 0).text);
    }
    assert!(answered, "the worker never answered; the log holds {:?}", log_of(&mut control, group));
}
