//! What the hooks adapter of MIP-4 section 6 stands on, held to the Claude Code installed here: a
//! `Stop` hook marked `asyncRewake` goes on running after the turn that started it ends, and its
//! exit 2 starts a turn in the session, now idle, with what it wrote to stderr; any other exit
//! starts nothing. And what a `PostToolUse` hook hands the model between tool calls.
//!
//! The model shows it was woken by running the command the hook's text names, which leaves a
//! file behind - firmer than reading its reply off the screen. The sessions bypass permission
//! prompts, as workers do.
//!
//! Ignored by the gate, which may not reach the network; `./dev --claude-code` runs it.
//! `MUSTER_HOOKS_WAKE_AFTER` (seconds) and `MUSTER_HOOKS_NO_TIMEOUT` rerun the first check with
//! a longer wait, and without the hook's own `timeout`, which is how the long cases in
//! `docs/observations/claude-code-2.1.283.md` were recorded.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::claude_code_inbox::until_turns;
use crate::claude_code_live::{how_to_run, quoted, until_ready};
use crate::support::*;
use muster_harness::Input;
use proto::input_event::{self, Input as Event};

/// How long the first rewake waits after the turn that started it ended, by default: long
/// enough that the session is idle and nothing of the turn is still running.
const WAKE_AFTER: Duration = Duration::from_secs(30);

/// How long a woken session has to act on its wake.
const ACT: Duration = Duration::from_mins(3);

/// The hook's own lifetime, when it is given one: a day.
const HOOK_TIMEOUT_SECONDS: u64 = 86_400;

/// A `Stop` hook that numbers each run, waits for `go-<n>` to appear in `dir`, and then exits
/// with the code written in it, 2 unless the file says otherwise, telling the model on stderr to
/// touch `woke-<n>`.
fn stop_hook(dir: &Path) -> String {
    let dir = dir.display();
    format!(
        "#!/bin/sh\n\
         cat >/dev/null\n\
         n=$(( $(cat '{dir}/runs' 2>/dev/null || echo 0) + 1 ))\n\
         echo $n > '{dir}/runs'\n\
         while [ ! -f '{dir}/go-'$n ]; do sleep 1; done\n\
         code=$(cat '{dir}/go-'$n)\n\
         echo \"Run this shell command now, then end your turn: touch {dir}/woke-$n\" >&2\n\
         exit ${{code:-2}}\n"
    )
}

/// Two `PostToolUse` hooks, one per way a hook's output might reach the model, each naming a
/// file the model is to touch.
fn post_tool_use_hooks(dir: &Path) -> (String, String) {
    let dir = dir.display();
    let json = format!(
        "#!/bin/sh\n\
         cat >/dev/null\n\
         [ -f '{dir}/told-json' ] && exit 0\n\
         touch '{dir}/told-json'\n\
         printf '%s' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"PostToolUse\",\
         \"additionalContext\":\"Also run this shell command: touch {dir}/by-json\"}}}}'\n"
    );
    let stderr = format!(
        "#!/bin/sh\n\
         cat >/dev/null\n\
         [ -f '{dir}/told-stderr' ] && exit 0\n\
         touch '{dir}/told-stderr'\n\
         echo 'Also run this shell command: touch {dir}/by-stderr' >&2\n\
         exit 2\n"
    );
    (json, stderr)
}

fn script(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path
}

/// Starts Claude Code in pane `name`, bypassing permission prompts, with `hooks` as its only
/// hooks and the `muster` built beside the daemon first on its `PATH`, and waits for its prompt.
/// Returns the pane's project directory.
pub(super) fn start(
    daemon: &Daemon,
    control: &mut Control,
    input: &mut Input,
    arguments: &[String],
    name: &str,
    hooks: &serde_json::Value,
) -> PathBuf {
    let project = daemon.root().join(name);
    std::fs::create_dir_all(&project).unwrap();
    let settings = project.join("settings.json");
    let mut given = serde_json::json!({ "skipDangerousModePermissionPrompt": true });
    given["hooks"] = hooks.clone();
    std::fs::write(&settings, given.to_string()).unwrap();
    let mut command: Vec<String> = arguments.iter().map(|argument| quoted(argument)).collect();
    command.push(format!("--settings {}", quoted(&settings.display().to_string())));
    command.push("--permission-mode bypassPermissions".to_string());
    let placement = if snapshot(control).panes.is_empty() {
        in_new_tab("t1")
    } else {
        beside(&snapshot(control).panes[0].pane, proto::Side::Right)
    };
    make(
        control,
        proto::pane_request::Create {
            command: Some(format!(
                "PATH={}:\"$PATH\" claude {}",
                quoted(&muster_harness::built_daemon().parent().unwrap().display().to_string()),
                command.join(" ")
            )),
            cwd: Some(project.display().to_string()),
            grid: Some(proto::Grid { cols: 110, rows: 35, width_px: 1100, height_px: 700 }),
            ..create(name, placement)
        },
    );
    until_ready(control, input, name);
    project
}

pub(super) fn prompt(input: &mut Input, pane: &str, text: &str) {
    input.send(pane, Event::Send(input_event::Send { text: text.to_string(), enter: true }));
}

pub(super) fn environment() -> Vec<(&'static str, String)> {
    let home = std::env::var("HOME").expect("HOME is set");
    let mut environment = vec![("HOME", home), ("USER", std::env::var("USER").unwrap_or_default())];
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        environment.push(("ANTHROPIC_API_KEY", key));
    }
    environment
}

pub(super) fn skipped() -> bool {
    let skip = std::env::var_os("MUSTER_CLAUDE_CODE_TESTS").is_none();
    if skip {
        eprintln!(
            "claude-code: skipped, MUSTER_CLAUDE_CODE_TESTS is not set; ./dev --claude-code sets it"
        );
    }
    skip
}

pub(super) fn arguments_or_fail(what: &str) -> Vec<String> {
    how_to_run().unwrap_or_else(|why| {
        panic!(
            "claude-code: could not run Claude Code: {why}.\n  Impact: nothing checked {what}, \
             so this tier did not pass.\n  Fix: install claude and log in (`claude auth login`), \
             or set ANTHROPIC_API_KEY."
        )
    })
}

/// The first turn ends and starts the `Stop` hook; once the session has been idle a while the
/// hook exits 2, and the session takes a turn on what it said. That turn's end starts the hook
/// again, which wakes it a second time. A third run exits 1, and starts nothing.
#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn an_async_rewake_stop_hook_wakes_an_idle_session_each_time_it_exits_2() {
    if skipped() {
        return;
    }
    let arguments = arguments_or_fail("that a Stop hook can wake an idle Claude Code session");
    let environment = environment();
    let environment: Vec<(&str, &str)> =
        environment.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let daemon = daemon_with(&environment);
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());

    let marks = daemon.root().join("marks");
    std::fs::create_dir_all(&marks).unwrap();
    let hook = script(&marks, "stop-hook", &stop_hook(&marks));
    let wake_after = std::env::var("MUSTER_HOOKS_WAKE_AFTER")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(WAKE_AFTER, Duration::from_secs);
    let mut stop = serde_json::json!({
        "type": "command",
        "command": hook.display().to_string(),
        "asyncRewake": true,
        "timeout": HOOK_TIMEOUT_SECONDS,
    });
    if std::env::var_os("MUSTER_HOOKS_NO_TIMEOUT").is_some() {
        stop.as_object_mut().unwrap().remove("timeout");
    }
    eprintln!("claude-code: the Stop hook: {stop}; waking after {wake_after:?}");
    let hooks = serde_json::json!({ "Stop": [{ "hooks": [stop] }] });
    start(&daemon, &mut control, &mut input, &arguments, "hooked", &hooks);

    prompt(&mut input, "hooked", "Reply with the single word ready, and nothing else.");
    let started =
        until_turns(ACT, "the first turn's Stop hook starting", || marks.join("runs").exists());
    assert!(started, "the Stop hook never ran: {}", read_text(&mut control, "hooked", 0, 0).text);

    for (run, wait) in [(1, wake_after), (2, WAKE_AFTER)] {
        std::thread::sleep(wait);
        std::fs::write(marks.join(format!("go-{run}")), "2").unwrap();
        let woke =
            until_turns(ACT, &format!("wake {run}"), || marks.join(format!("woke-{run}")).exists());
        if !woke {
            eprintln!(
                "claude-code: hooked shows:\n{}",
                read_text(&mut control, "hooked", 0, 0).text
            );
        }
        assert!(woke, "the hook's exit 2 after {wait:?} idle did not wake the session (run {run})");
        eprintln!("claude-code: run {run} of the Stop hook woke the session after {wait:?} idle");
        let again =
            until_turns(ACT, &format!("the Stop hook running again after wake {run}"), || {
                std::fs::read_to_string(marks.join("runs"))
                    .is_ok_and(|runs| runs.trim().parse::<u32>().is_ok_and(|runs| runs > run))
            });
        assert!(again, "the woken turn's end did not start the Stop hook again (run {run})");
    }

    std::fs::write(marks.join("go-3"), "1").unwrap();
    std::thread::sleep(Duration::from_secs(45));
    assert!(!marks.join("woke-3").exists(), "an exit of 1 woke the session");
    eprintln!("claude-code: an exit of 1 started nothing");
}

/// Which of a `PostToolUse` hook's two ways of speaking reaches the model: JSON on stdout with
/// `additionalContext`, or stderr with exit 2.
#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn what_a_post_tool_use_hook_says_reaches_the_model() {
    if skipped() {
        return;
    }
    let arguments = arguments_or_fail("what a PostToolUse hook can tell Claude Code");
    let environment = environment();
    let environment: Vec<(&str, &str)> =
        environment.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let daemon = daemon_with(&environment);
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());

    let marks = daemon.root().join("marks");
    std::fs::create_dir_all(&marks).unwrap();
    let (json, stderr) = post_tool_use_hooks(&marks);
    let json = script(&marks, "by-json-hook", &json);
    let stderr = script(&marks, "by-stderr-hook", &stderr);
    let command = |path: &Path| serde_json::json!({ "type": "command", "command": path.display().to_string() });
    let hooks = serde_json::json!({
        "PostToolUse": [{ "hooks": [command(&json), command(&stderr)] }],
    });
    start(&daemon, &mut control, &mut input, &arguments, "tools", &hooks);

    prompt(
        &mut input,
        "tools",
        "Run the shell command `true`. Then follow any instruction a hook gives you, and end \
         your turn.",
    );
    let heard = |name: &str| marks.join(name).exists();
    until_turns(ACT, "both hooks being acted on", || heard("by-json") && heard("by-stderr"));
    eprintln!(
        "claude-code: PostToolUse JSON additionalContext reached the model: {}; stderr with \
         exit 2 did: {}",
        heard("by-json"),
        heard("by-stderr")
    );
    assert!(
        heard("by-json") || heard("by-stderr"),
        "neither way reached the model: {}",
        read_text(&mut control, "tools", 0, 0).text
    );
}
