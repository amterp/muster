//! The Claude Code installed on this machine, in two panes of a daemon, driven for one short
//! turn: the pane with Muster's hooks reads working and then idle from Claude Code's own
//! reports, and the pane without them reads the same off its screen. This is what says whether
//! a Claude Code update has broken either path. The same two panes, narrow and in plan mode,
//! read blocked at the dialog asking to go ahead with a plan. Both panes compact when the daemon
//! asks. A pane of its own checks that the pane's name and the session's follow each other, and
//! another that a hooked session comes back resumed after a daemon restart.
//!
//! Out of the gate, because it reaches the network and spends a turn of a real model. It runs
//! with `ANTHROPIC_API_KEY` if that is set, and otherwise with the login `claude` already has;
//! with neither, or with no `claude` on the PATH, it fails and says why, since a tier asked for
//! that checked nothing has not passed. `./dev --claude-code` runs it; any other run of the
//! ignored tests passes it by without trying.

use std::process::Command;
use std::time::{Duration, Instant};

use crate::support::*;
use muster_harness::Input;
use proto::input_event::{self, Input as Event};

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/claude-code");

/// Long enough for Claude Code to start, answer one short prompt and settle.
const TURN: Duration = Duration::from_mins(3);

/// How this machine can run Claude Code without anyone's own settings getting in the way: an
/// API key with `--bare`, which loads no settings, hooks or plugins at all; or the login it
/// has, with only the project's settings, of which a scratch directory has none.
pub(super) fn how_to_run() -> Result<Vec<String>, String> {
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

pub(super) fn quoted(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', r"'\''"))
}

/// Every change to each pane's record, in order, until every one has gone working and then idle.
pub(super) fn until_settled<const N: usize>(
    control: &mut Control,
    panes: [&str; N],
) -> [Vec<proto::Pane>; N] {
    let settled = |seen: &[proto::Pane]| {
        let working =
            seen.iter().position(|record| record.agent_state() == proto::AgentState::Working);
        working.is_some_and(|at| {
            seen[at..].iter().any(|record| record.agent_state() == proto::AgentState::Idle)
        })
    };
    let deadline = Instant::now() + TURN;
    let mut seen = std::array::from_fn(|_| Vec::new());
    while !seen.iter().all(|seen| settled(seen)) {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            for pane in panes {
                eprintln!("claude-code: {pane} shows:\n{}", read_text(control, pane, 0, 0).text);
            }
            panic!("the panes did not all settle within {TURN:?}: {seen:?}");
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
/// asks.
pub(super) fn until_ready(control: &mut Control, input: &mut Input, pane: &str) {
    let deadline = Instant::now() + TURN;
    loop {
        let screen = read_text(control, pane, 0, 0).text;
        if screen.contains("trust") && screen.contains("folder") {
            input.send(
                pane,
                Event::Send(input_event::Send {
                    text: String::new(),
                    enter: true,
                    ..Default::default()
                }),
            );
        } else if screen.contains("? for shortcuts") || screen.contains('❯') {
            break;
        }
        assert!(Instant::now() < deadline, "{pane}: Claude Code never showed its prompt: {screen}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Waits for Claude Code's prompt and sends it one.
fn prompt(control: &mut Control, input: &mut Input, pane: &str) {
    until_ready(control, input, pane);
    // Past the grace a newly found agent is held idle through, which Claude Code's start can
    // outlast by less than a short turn takes.
    std::thread::sleep(Duration::from_secs(4));
    let text =
        "Without using any tools, write the numbers from 1 to 80, one per line, and nothing else."
            .to_string();
    input.send(pane, Event::Send(input_event::Send { text, enter: true, ..Default::default() }));
}

/// A daemon for a live check, with `project` made under its root, and the arguments Claude Code
/// runs with here; None when the tier is not asked for. The daemon's environment carries this
/// user's home, where Claude Code keeps its login, and a restart starts it with the same.
fn live_daemon() -> Option<(Daemon, Vec<String>)> {
    if std::env::var_os("MUSTER_CLAUDE_CODE_TESTS").is_none() {
        eprintln!(
            "claude-code: skipped, MUSTER_CLAUDE_CODE_TESTS is not set; ./dev --claude-code sets it"
        );
        return None;
    }
    let arguments = how_to_run().unwrap_or_else(|why| {
        panic!(
            "claude-code: could not run Claude Code: {why}.\n  Impact: nothing checked that \
             Claude Code's hooks and screen still read its states, so this tier did not pass.\n  \
             Fix: install claude and log in (`claude auth login`), or set ANTHROPIC_API_KEY."
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
    std::fs::create_dir_all(daemon.root().join("project")).unwrap();
    let arguments = arguments.iter().map(|argument| quoted(argument)).collect();
    Some((daemon, arguments))
}

/// The two panes, `hooked` with Muster's hooks and `screen` without, side by side, each running
/// Claude Code with `extra` arguments in a pane `grid` large; None when the tier is not asked for.
fn two_panes(extra: &str, grid: proto::Grid) -> Option<(Daemon, Control, Input)> {
    let (daemon, arguments) = live_daemon()?;
    let project = daemon.root().join("project");

    let mut control = daemon.connect();
    control.ask(subscribe_request());
    let panes = [("hooked", format!("--plugin-dir {}", quoted(PLUGIN))), ("screen", String::new())];
    for (index, (name, hooks)) in panes.iter().enumerate() {
        let command = format!("claude {} {hooks} {extra}", arguments.join(" "));
        let placement =
            if index == 0 { in_new_tab("t1") } else { beside("hooked", proto::Side::Right) };
        make(
            &mut control,
            proto::pane_request::Create {
                command: Some(command),
                cwd: Some(project.display().to_string()),
                grid: Some(grid),
                ..create(name, placement)
            },
        );
    }
    let input = Input::connect(daemon.socket_path());
    Some((daemon, control, input))
}

const PANES: [&str; 2] = ["hooked", "screen"];

#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn claude_code_reads_working_then_idle_through_its_hooks_and_through_its_screen() {
    let grid = proto::Grid { cols: 100, rows: 30, width_px: 1000, height_px: 600 };
    let Some((_daemon, mut control, mut input)) = two_panes("", grid) else { return };
    for name in PANES {
        prompt(&mut control, &mut input, name);
    }
    let settled = until_settled(&mut control, PANES);
    for (name, seen) in PANES.into_iter().zip(settled) {
        let summary: Vec<_> = seen
            .iter()
            .map(|record| (record.agent_state(), record.state_reported, record.screen_unreadable))
            .collect();
        eprintln!("claude-code: {name}: {summary:?}");
        let hooked = name == "hooked";
        let working = seen.iter().find(|record| record.agent_state() == proto::AgentState::Working);
        let idle = seen.iter().rev().find(|record| record.agent_state() == proto::AgentState::Idle);
        assert_eq!(working.map(|record| record.state_reported), Some(hooked), "{name}: working");
        assert_eq!(idle.map(|record| record.state_reported), Some(hooked), "{name}: idle");
        assert!(seen.iter().all(|record| !record.screen_unreadable), "{name}: {summary:?}");
        assert_eq!(seen.last().and_then(|record| record.agent.clone()).as_deref(), Some("claude"));
    }
}

/// A session at plan mode's dialog asking whether to go ahead with its plan is waiting on a
/// person. The panes are narrow, so the dialog's question wraps, which is where it was read idle.
#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn claude_code_at_its_plan_approval_dialog_reads_blocked_through_both_paths() {
    let grid = proto::Grid { cols: 66, rows: 40, width_px: 660, height_px: 800 };
    let Some((daemon, mut control, mut input)) = two_panes("--permission-mode plan", grid) else {
        return;
    };
    for name in PANES {
        until_ready(&mut control, &mut input, name);
    }
    std::thread::sleep(Duration::from_secs(4));
    let text = "Plan to create a file named hello.txt holding the word hi. The plan is one line. \
                Do not explore anything: call the ExitPlanMode tool with that plan right away."
        .to_string();
    for name in PANES {
        input.send(
            name,
            Event::Send(input_event::Send {
                text: text.clone(),
                enter: true,
                ..Default::default()
            }),
        );
    }
    let mut looking = daemon.connect();
    let deadline = Instant::now() + TURN;
    for name in PANES {
        loop {
            let screen = read_text(&mut looking, name, 0, 0).text;
            if screen.contains("proceed?") {
                break;
            }
            assert!(Instant::now() < deadline, "{name}: no plan dialog came: {screen}");
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    // Long past a working report's ten quiet seconds, which is when a dialog the rules did not
    // read gave way to the idle title.
    std::thread::sleep(Duration::from_secs(12));
    for record in snapshot(&mut looking).panes {
        let screen = read_text(&mut looking, &record.pane, 0, 0).text;
        eprintln!(
            "claude-code: {}: {:?}, reported {}",
            record.pane,
            record.agent_state(),
            record.state_reported
        );
        assert_eq!(record.agent_state(), proto::AgentState::Blocked, "{}: {screen}", record.pane);
        assert_eq!(record.state_reported, record.pane == "hooked", "{}", record.pane);
    }
}

/// A pane named when it is made names the session started in it, and a session renamed in
/// Claude Code renames the pane, through the statusline in `extras/claude-code`. No turn is
/// taken: `/rename` is Claude Code's own.
#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn a_pane_and_its_claude_code_session_take_each_others_names() {
    let Some((daemon, arguments)) = live_daemon() else { return };
    let project = daemon.root().join("project");
    let statusline = format!("{PLUGIN}/statusline.sh");
    let settings = serde_json::json!({
        "statusLine": { "type": "command", "refreshInterval": 1, "command": statusline }
    })
    .to_string();
    let command = format!("claude {} --settings {}", arguments.join(" "), quoted(&settings));
    let mut control = daemon.connect();
    make(
        &mut control,
        proto::pane_request::Create {
            command: Some(command),
            cwd: Some(project.display().to_string()),
            label: Some("🤖 live".to_string()),
            ..create("named", in_new_tab("t1"))
        },
    );
    let mut input = Input::connect(daemon.socket_path());
    until_ready(&mut control, &mut input, "named");

    let label = |control: &mut Control| {
        snapshot(control).panes.into_iter().find(|record| record.pane == "named")?.label
    };
    let deadline = Instant::now() + TURN;
    // Claude Code draws a session's name at the right end of the rule above its prompt box.
    loop {
        let screen = read_text(&mut control, "named", 0, 0).text;
        if screen.lines().any(|line| line.contains("─ 🤖 live ─")) {
            break;
        }
        assert!(Instant::now() < deadline, "the session never took the pane's name: {screen}");
        std::thread::sleep(Duration::from_millis(500));
    }

    std::thread::sleep(Duration::from_secs(4));
    let text = "/rename named in claude".to_string();
    input.send("named", Event::Send(input_event::Send { text, enter: true, ..Default::default() }));
    loop {
        if label(&mut control).as_deref() == Some("named in claude") {
            break;
        }
        let screen = read_text(&mut control, "named", 0, 0).text;
        assert!(Instant::now() < deadline, "the pane never took the session's name: {screen}");
        std::thread::sleep(Duration::from_millis(500));
    }
    // Nothing typed back: the name stays, and Claude Code's transcript holds the two renames.
    std::thread::sleep(Duration::from_secs(8));
    assert_eq!(label(&mut control).as_deref(), Some("named in claude"));
    let screen = read_text(&mut control, "named", 0, 0).text;
    let renames: Vec<&str> = screen.lines().filter(|line| line.starts_with("❯ /rename")).collect();
    assert_eq!(renames, ["❯ /rename 🤖 live", "❯ /rename named in claude"], "{screen}");
}

/// What `muster pane compact` types compacts a real session, which says Claude Code still takes
/// `/compact {focus}` (`claude.toml`'s `[session] compact`). Both panes, since the daemon types
/// only at an idle, empty prompt, and one pane learns that from hooks and the other from its
/// screen. Claude Code confirms with a line of its own under the command.
#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn claude_code_compacts_when_the_daemon_asks_through_both_paths() {
    let grid = proto::Grid { cols: 100, rows: 30, width_px: 1000, height_px: 600 };
    let Some((daemon, mut control, mut input)) = two_panes("", grid) else { return };
    // A session with no conversation has nothing to compact.
    for name in PANES {
        prompt(&mut control, &mut input, name);
    }
    until_settled(&mut control, PANES);

    let mut asking = daemon.connect();
    for name in PANES {
        let compact = proto::pane_request::Compact {
            pane: name.to_string(),
            focus: Some("keep the numbers".to_string()),
        };
        let asked = asking.ask(pane(proto::pane_request::Request::Compact(compact)));
        assert_eq!(asked.outcome(), proto::Outcome::Done, "{name}: {}", asked.answer.reason);
    }
    let deadline = Instant::now() + TURN;
    for name in PANES {
        loop {
            let screen = read_text(&mut asking, name, 0, 0).text;
            let mut after_command = screen
                .lines()
                .skip_while(|line| !line.starts_with("❯ /compact keep the numbers"))
                .skip(1);
            if after_command.any(|line| line.contains("Compacted")) {
                break;
            }
            assert!(Instant::now() < deadline, "{name}: the session never compacted: {screen}");
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

/// A session reported by its hooks comes back after a daemon restart as
/// `claude <its flags> --resume <id>`, which says Claude Code's SessionStart still hands the
/// hook its id and `--resume` still takes it (`claude.toml`'s `[session] resume`). `--model
/// haiku`, from [`how_to_run`], is the flag that has to go along. The resumed pane shows the
/// first turn's prompt, so it is that conversation and not a new one.
#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn a_claude_code_session_comes_back_resumed_after_a_daemon_restart() {
    use crate::persistence::{record, state_file, stop, until_restored, until_saved};

    let Some((mut daemon, arguments)) = live_daemon() else { return };
    let project = daemon.root().join("project");
    let mut control = daemon.connect();
    control.ask(subscribe_request());
    let command = format!("claude {} --plugin-dir {}", arguments.join(" "), quoted(PLUGIN));
    make(
        &mut control,
        proto::pane_request::Create {
            command: Some(command),
            cwd: Some(project.display().to_string()),
            ..create("hooked", in_new_tab("t1"))
        },
    );
    let mut input = Input::connect(daemon.socket_path());
    prompt(&mut control, &mut input, "hooked");
    until_settled(&mut control, ["hooked"]);

    let saved_session = |daemon: &Daemon| -> Option<String> {
        let state: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(state_file(daemon)).ok()?).ok()?;
        let session = &state["panes"].as_array()?.first()?["resume"]["session"];
        session.as_str().map(str::to_string)
    };
    until_saved(&daemon, "\"resume\"");
    let session = saved_session(&daemon).expect("the state file holds the pane's resume");
    eprintln!("claude-code: the hooks reported session {session}");

    stop(&mut daemon, &mut control);
    daemon.restart();
    let mut control = daemon.connect();
    let restored = until_restored(&mut control, 1);
    let command = record(&restored, "hooked").command.clone().unwrap_or_default();
    eprintln!("claude-code: restored as {command}");
    assert!(command.starts_with("claude "), "{command}");
    assert!(command.contains(" --model haiku "), "--model haiku did not go along: {command}");
    assert!(command.ends_with(&format!(" --resume {session}")), "{command}");

    let mut input = Input::connect(daemon.socket_path());
    until_ready(&mut control, &mut input, "hooked");
    let deadline = Instant::now() + TURN;
    loop {
        let screen = read_text(&mut control, "hooked", 0, 0).text;
        let agent = record(&snapshot(&mut control), "hooked").agent.clone();
        if agent.as_deref() == Some("claude") && screen.contains("write the numbers from 1 to 80") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the resumed pane read as {agent:?} or showed no earlier conversation: {screen}"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    // Whether Claude Code keeps the id on --resume decides which session a second restart
    // resumes; noted rather than held, since either resumes the conversation.
    std::thread::sleep(Duration::from_secs(4));
    eprintln!("claude-code: after the resume the saved session is {:?}", saved_session(&daemon));
}
