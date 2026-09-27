//! The Claude Code installed on this machine, in two panes of a daemon, driven for one short
//! turn: the pane with Muster's hooks reads working and then idle from Claude Code's own
//! reports, and the pane without them reads the same off its screen. This is what says whether
//! a Claude Code update has broken either path.
//!
//! Out of the gate, because it reaches the network and spends a turn of a real model. It runs
//! with `ANTHROPIC_API_KEY` if that is set, and otherwise with the login `claude` already has;
//! with neither, or with no `claude` on the PATH, it says why and passes. `./dev --claude-code`
//! runs it.

mod support;

use std::process::Command;
use std::time::{Duration, Instant};

use muster_harness::Input;
use proto::input_event::{self, Input as Event};
use support::*;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/claude-code");

/// Long enough for Claude Code to start, answer one short prompt and settle.
const TURN: Duration = Duration::from_mins(3);

/// How this machine can run Claude Code without anyone's own settings getting in the way: an
/// API key with `--bare`, which loads no settings, hooks or plugins at all; or the login it
/// has, with only the project's settings, of which a scratch directory has none.
fn how_to_run() -> Result<Vec<String>, String> {
    if std::env::var_os("MUSTER_CLAUDE_CODE_TESTS").is_none() {
        return Err("MUSTER_CLAUDE_CODE_TESTS is not set; ./dev --claude-code sets it".to_string());
    }
    let version = Command::new("claude")
        .arg("--version")
        .output()
        .map_err(|_| "no claude on the PATH".to_string())?;
    eprintln!("claude-code: {}", String::from_utf8_lossy(&version.stdout).trim());
    let isolated = if std::env::var_os("ANTHROPIC_API_KEY").is_some() {
        eprintln!("claude-code: using ANTHROPIC_API_KEY with --bare");
        vec!["--bare".to_string()]
    } else {
        let status = Command::new("claude").args(["auth", "status"]).output().ok();
        let logged_in = status
            .and_then(|status| serde_json::from_slice::<serde_json::Value>(&status.stdout).ok())
            .is_some_and(|status| status["loggedIn"] == true);
        if !logged_in {
            return Err("no ANTHROPIC_API_KEY and claude is not logged in".to_string());
        }
        eprintln!("claude-code: using claude's own login, with project settings only");
        vec!["--setting-sources".to_string(), "project".to_string()]
    };
    Ok([isolated, vec!["--model".to_string(), "haiku".to_string()]].concat())
}

fn quoted(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', r"'\''"))
}

/// Every change to either pane's record, in order, until both have gone working and then idle.
fn until_both_settle(control: &mut Control, panes: [&str; 2]) -> [Vec<proto::Pane>; 2] {
    let settled = |seen: &[proto::Pane]| {
        let working =
            seen.iter().position(|record| record.agent_state() == proto::AgentState::Working);
        working.is_some_and(|at| {
            seen[at..].iter().any(|record| record.agent_state() == proto::AgentState::Idle)
        })
    };
    let deadline = Instant::now() + TURN;
    let mut seen = [Vec::new(), Vec::new()];
    while !seen.iter().all(|seen| settled(seen)) {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            for pane in panes {
                eprintln!("claude-code: {pane} shows:\n{}", read_text(control, pane, 0, 0).text);
            }
            panic!("the panes did not both settle within {TURN:?}: {seen:?}");
        };
        if let Some(proto::control_message::Message::Event(event)) = control.next_message(left)
            && let Some(proto::event::Event::PaneChanged(changed)) = event.event
            && let Some(record) = changed.pane
            && let Some(index) = panes.iter().position(|pane| *pane == record.pane)
        {
            seen[index].push(record);
        }
    }
    seen
}

/// Waits for Claude Code's prompt, answering the question about trusting a new folder if it
/// asks, and sends it a prompt.
fn prompt(control: &mut Control, input: &mut Input, pane: &str) {
    let deadline = Instant::now() + TURN;
    loop {
        let screen = read_text(control, pane, 0, 0).text;
        if screen.contains("trust") && screen.contains("folder") {
            input.send(pane, Event::Send(input_event::Send { text: String::new(), enter: true }));
        } else if screen.contains("? for shortcuts") || screen.contains('❯') {
            break;
        }
        assert!(Instant::now() < deadline, "{pane}: Claude Code never showed its prompt: {screen}");
        std::thread::sleep(Duration::from_millis(500));
    }
    // Past the grace a newly found agent is held idle through, which Claude Code's start can
    // outlast by less than a short turn takes.
    std::thread::sleep(Duration::from_secs(4));
    let text = "Write the numbers from 1 to 80, one per line, and nothing else.".to_string();
    input.send(pane, Event::Send(input_event::Send { text, enter: true }));
}

#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn claude_code_reads_working_then_idle_through_its_hooks_and_through_its_screen() {
    let arguments = match how_to_run() {
        Ok(arguments) => arguments,
        Err(why) => {
            eprintln!("claude-code: skipped, {why}");
            return;
        }
    };
    let home = std::env::var("HOME").expect("HOME is set");
    let mut environment = vec![("HOME", home), ("USER", std::env::var("USER").unwrap_or_default())];
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        environment.push(("ANTHROPIC_API_KEY", key));
    }
    let environment: Vec<(&str, &str)> =
        environment.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let daemon = daemon_with(&environment);
    let project = daemon.root().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let mut control = daemon.connect();
    control.ask(subscribe_request());
    let arguments: Vec<String> = arguments.iter().map(|argument| quoted(argument)).collect();
    let panes = [("hooked", format!("--plugin-dir {}", quoted(PLUGIN))), ("screen", String::new())];
    for (index, (name, extra)) in panes.iter().enumerate() {
        let command = format!("claude {} {extra}", arguments.join(" "));
        let placement =
            if index == 0 { in_new_tab("t1") } else { beside("hooked", proto::Side::Right) };
        make(
            &mut control,
            proto::pane_request::Create {
                command: Some(command),
                cwd: Some(project.display().to_string()),
                grid: Some(proto::Grid { cols: 100, rows: 30, width_px: 1000, height_px: 600 }),
                ..create(name, placement)
            },
        );
    }

    let mut input = Input::connect(daemon.socket_path());
    for (name, _) in &panes {
        prompt(&mut control, &mut input, name);
    }
    let settled = until_both_settle(&mut control, ["hooked", "screen"]);
    for ((name, _), seen) in panes.iter().zip(settled) {
        let summary: Vec<_> = seen
            .iter()
            .map(|record| (record.agent_state(), record.state_reported, record.screen_unreadable))
            .collect();
        eprintln!("claude-code: {name}: {summary:?}");
        let hooked = *name == "hooked";
        let working = seen.iter().find(|record| record.agent_state() == proto::AgentState::Working);
        let idle = seen.iter().rev().find(|record| record.agent_state() == proto::AgentState::Idle);
        assert_eq!(working.map(|record| record.state_reported), Some(hooked), "{name}: working");
        assert_eq!(idle.map(|record| record.state_reported), Some(hooked), "{name}: idle");
        assert!(seen.iter().all(|record| !record.screen_unreadable), "{name}: {summary:?}");
        assert_eq!(seen.last().and_then(|record| record.agent.clone()).as_deref(), Some("claude"));
    }
}
