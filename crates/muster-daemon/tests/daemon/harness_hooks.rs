//! The hooks each harness's wiring in `extras/` installs, run as the harness runs them: what each
//! event reports to the daemon owning the pane. A harness's hooks are the only way its own word
//! reaches Muster, so every event a file names is pinned here, and every event that ends a turn
//! must report idle - an agent that declared a wait is moved on only by its own idle report.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use super::claude_code::Scratch;

struct Harness {
    /// Its directory under `extras/`.
    extras: &'static str,
    hooks: &'static str,
    /// Each event its hooks answer, with the arguments `$MUSTER_DAEMON` is run with, one after
    /// another where the event runs it more than once.
    reports: &'static [(&'static str, &'static str)],
    /// The events that end a turn.
    turn_ends: &'static [&'static str],
}

const CLAUDE_CODE: Harness = Harness {
    extras: "claude-code",
    hooks: include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../extras/claude-code/hooks/hooks.json"
    )),
    reports: &[
        ("SessionStart", "[report][--clear][report][--agent][claude][--session-id-from-hook]"),
        ("SubagentStart", "[report][--subagent-started]"),
        ("SubagentStop", "[report][--subagent-stopped]"),
        ("UserPromptSubmit", "[report][--agent][claude][--state][working][--waiting][]"),
        ("PostToolUse", "[report][--agent][claude][--state][working]"),
        ("PostToolUseFailure", "[report][--agent][claude][--state][working]"),
        ("PermissionRequest", "[report][--agent][claude][--state][blocked]"),
        ("Notification", "[report][--agent][claude][--state][blocked]"),
        ("Stop", "[report][--agent][claude][--state][idle]"),
        ("StopFailure", "[report][--agent][claude][--state][idle]"),
    ],
    turn_ends: &["Stop", "StopFailure"],
};

/// Codex has no `Notification`, and `Interrupt` is how a turn Esc ended says so: no `Stop`
/// follows it (docs/observations/codex-0.154.0.md).
const CODEX: Harness = Harness {
    extras: "codex",
    hooks: include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../extras/codex/hooks/hooks.json"
    )),
    reports: &[
        ("SessionStart", "[report][--clear]"),
        ("SubagentStart", "[report][--subagent-started]"),
        ("SubagentStop", "[report][--subagent-stopped]"),
        ("UserPromptSubmit", "[report][--agent][codex][--state][working][--waiting][]"),
        ("PostToolUse", "[report][--agent][codex][--state][working]"),
        ("PermissionRequest", "[report][--agent][codex][--state][blocked]"),
        ("Stop", "[report][--agent][codex][--state][idle]"),
        ("Interrupt", "[report][--agent][codex][--state][idle]"),
    ],
    turn_ends: &["Stop", "Interrupt"],
};

const HARNESSES: [Harness; 2] = [CLAUDE_CODE, CODEX];

/// Each hook command the file gives `event`, in order.
fn commands(harness: &Harness, event: &str) -> Vec<String> {
    let hooks: serde_json::Value = serde_json::from_str(harness.hooks).unwrap();
    hooks["hooks"][event]
        .as_array()
        .unwrap_or_else(|| panic!("{}: no {event} hook", harness.extras))
        .iter()
        .flat_map(|group| group["hooks"].as_array().unwrap().iter())
        .map(|hook| hook["command"].as_str().unwrap().to_string())
        .collect()
}

/// The shells a harness may run a hook command in: `sh`, and the user's own, which on a Mac is
/// zsh - where an unquoted expansion is not split into words, as it is in `sh`. Codex runs its
/// hooks in the user's shell.
fn shells() -> Vec<&'static str> {
    ["/bin/sh", "/bin/zsh"]
        .into_iter()
        .filter(|shell| std::path::Path::new(shell).exists())
        .collect()
}

/// What `command` runs `$MUSTER_DAEMON` with, bracketed per argument, in each of [`shells`],
/// which must agree; empty when it does not run it.
fn reported(command: &str, scratch: &Scratch) -> String {
    let said: Vec<String> =
        shells().iter().map(|shell| reported_in(shell, command, scratch)).collect();
    assert!(
        said.windows(2).all(|pair| pair[0] == pair[1]),
        "the shells disagree on {command}: {said:?}"
    );
    said.into_iter().next().unwrap_or_default()
}

fn reported_in(shell: &str, command: &str, scratch: &Scratch) -> String {
    let arguments = scratch.0.join("arguments");
    let _ = std::fs::remove_file(&arguments);
    let daemon = scratch.0.join("daemon");
    // Written aside and moved into place: a test waits for the file to appear, and a redirect
    // makes it before printf fills it.
    std::fs::write(
        &daemon,
        format!(
            "#!/bin/sh\nprintf '[%s]' \"$@\" > '{0}.part' && mv '{0}.part' '{0}'\n",
            arguments.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ran = Command::new(shell)
        .args(["-c", command])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("MUSTER_DAEMON", &daemon)
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(ran.success(), "a hook never fails a session: {command}");
    std::fs::read_to_string(&arguments).unwrap_or_default()
}

#[test]
fn every_hook_reports_what_its_event_means_and_every_turn_end_reports_idle() {
    let scratch = Scratch::new("harness-hooks");
    for harness in &HARNESSES {
        let hooks: serde_json::Value = serde_json::from_str(harness.hooks).unwrap();
        let mut events: Vec<&str> =
            hooks["hooks"].as_object().unwrap().keys().map(String::as_str).collect();
        let mut pinned: Vec<&str> = harness.reports.iter().map(|(event, _)| *event).collect();
        events.sort_unstable();
        pinned.sort_unstable();
        assert_eq!(events, pinned, "{}: every event its hooks answer is pinned", harness.extras);
        for (event, expected) in harness.reports {
            let said: Vec<String> = commands(harness, event)
                .iter()
                .map(|command| reported(command, &scratch))
                .filter(|said| !said.is_empty())
                .collect();
            assert_eq!(said.concat(), *expected, "{}: {event}", harness.extras);
        }
        for event in harness.turn_ends {
            let idle = harness.reports.iter().find(|(reported, _)| reported == event);
            assert!(
                idle.is_some_and(|(_, said)| said.ends_with("[--state][idle]")),
                "{}: {event} ends a turn and does not report idle",
                harness.extras
            );
        }
    }
}

/// What the SessionStart hook with no matcher prints, which the harness adds to the session's
/// context, when run in an environment holding `variables`.
fn session_start_context(harness: &Harness, variables: &[(&str, &str)]) -> String {
    let hooks: serde_json::Value = serde_json::from_str(harness.hooks).unwrap();
    let groups = hooks["hooks"]["SessionStart"].as_array().expect("a SessionStart hook");
    let every_session = groups.iter().find(|group| group.get("matcher").is_none());
    let command = every_session.expect("one for every session")["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_string();
    let ran = Command::new("/bin/sh")
        .args(["-c", &command])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .envs(variables.iter().copied())
        .output()
        .unwrap();
    assert!(ran.status.success(), "the hook never fails a session");
    String::from_utf8(ran.stdout).unwrap()
}

/// An agent in a Muster pane is told, in one line, how to say it is waiting on its own work.
/// Nowhere else: the line costs every session some context, and outside Muster it means nothing.
#[test]
fn only_a_session_in_a_muster_pane_is_told_how_to_say_it_is_waiting() {
    for harness in &HARNESSES {
        let in_a_pane = [("MUSTER_PANE", "p1"), ("MUSTER_DAEMON", "/bin/true")];
        let told = session_start_context(harness, &in_a_pane);
        assert_eq!(told.lines().count(), 1, "{}: {told}", harness.extras);
        assert!(told.contains("report --waiting"), "{}: {told}", harness.extras);
        assert_eq!(session_start_context(harness, &[("MUSTER_DAEMON", "/bin/true")]), "");
        assert_eq!(session_start_context(harness, &[]), "");
    }
}

/// The tail of a Codex 0.154.0 rollout transcript: two token counts with an answer between them.
const ROLLOUT_TAIL: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/codex-0.154.0/rollout-tail.jsonl");

/// Codex has no statusline, so its hooks read how full its context is from the last token count
/// in its transcript, as Codex's own "N% context left" counts it: past a baseline of 12,000
/// tokens it does not count as used. The last of the tail's counts is 19,313 tokens of 258,400.
#[test]
fn codexs_hooks_report_its_context_from_its_transcripts_last_token_count() {
    let scratch = Scratch::new("codex-context");
    let arguments = scratch.0.join("arguments");
    let daemon = scratch.0.join("daemon");
    // Written aside and moved into place: a test waits for the file to appear, and a redirect
    // makes it before printf fills it.
    std::fs::write(
        &daemon,
        format!(
            "#!/bin/sh\nprintf '[%s]' \"$@\" > '{0}.part' && mv '{0}.part' '{0}'\n",
            arguments.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
    let input = serde_json::json!({ "transcript_path": ROLLOUT_TAIL, "model": "gpt-5.6-luna" });
    for (event, shell) in ["PostToolUse", "Stop"]
        .into_iter()
        .flat_map(|event| shells().into_iter().map(move |shell| (event, shell)))
    {
        let context = commands(&CODEX, event)
            .into_iter()
            .find(|command| command.contains("token_count"))
            .unwrap_or_else(|| panic!("no {event} hook reads the context"));
        let _ = std::fs::remove_file(&arguments);
        let mut hook = Command::new(shell)
            .args(["-c", &context])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("MUSTER_DAEMON", &daemon)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        hook.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        assert!(hook.wait().unwrap().success(), "a hook never fails a session");
        muster_harness::until_within(
            "the context report, which runs in the background",
            Duration::from_secs(20),
            || arguments.exists(),
            || format!("{event}: nothing reported"),
        );
        let said = std::fs::read_to_string(&arguments).unwrap();
        assert!(said.starts_with("[report][--context-used][2.96"), "{event} in {shell}: {said}");
        assert!(said.ends_with("[--model][gpt-5.6-luna]"), "{event} in {shell}: {said}");
    }
}

/// Codex is woken through `codex queue` by its session's id, which only its hooks are handed:
/// `SessionStart` hands the daemon its input to read the id out of, needing no jq, and says
/// nothing to the model doing so.
#[test]
fn codexs_session_start_reports_its_session_id() {
    let scratch = Scratch::new("codex-session-id");
    let arguments = scratch.0.join("arguments");
    let given = scratch.0.join("stdin");
    let daemon = scratch.0.join("daemon");
    std::fs::write(
        &daemon,
        format!(
            "#!/bin/sh\nprintf '[%s]' \"$@\" >> '{}'\ncat > '{}'\n",
            arguments.display(),
            given.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
    let input = serde_json::json!({ "session_id": "019a-b c", "source": "startup" });
    let reporting = commands(&CODEX, "SessionStart")
        .into_iter()
        .find(|command| command.contains("session-id"))
        .expect("a SessionStart hook reports the session's id");
    for shell in shells() {
        let _ = std::fs::remove_file(&arguments);
        let mut hook = Command::new(shell)
            .args(["-c", &reporting])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("MUSTER_DAEMON", &daemon)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        hook.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let output = hook.wait_with_output().unwrap();
        assert!(output.status.success(), "a hook never fails a session");
        assert!(output.stdout.is_empty(), "{shell}: what it prints reaches the model");
        let said = std::fs::read_to_string(&arguments).unwrap_or_default();
        assert_eq!(said, "[report][--agent][codex][--from][session-id=/session_id]", "in {shell}");
        let handed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&given).unwrap()).unwrap();
        assert_eq!(handed, input, "in {shell}: the daemon reads the id from the hook's input");
    }
}

/// Claude Code's session is resumed after a restart by its id, which only its hooks are handed:
/// `SessionStart`, on every source a session starts from, has the report read it from the hook's
/// input, so the hook needs no JSON tool, and says nothing to the model doing so.
#[test]
fn claude_codes_session_start_reports_its_session_id() {
    let scratch = Scratch::new("claude-session-id");
    let arguments = scratch.0.join("arguments");
    let read = scratch.0.join("read");
    let daemon = scratch.0.join("daemon");
    std::fs::write(
        &daemon,
        format!(
            "#!/bin/sh\nprintf '[%s]' \"$@\" >> '{}'\ncat > '{}'\n",
            arguments.display(),
            read.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
    let input = serde_json::json!({ "session_id": "0199-a b", "source": "resume" }).to_string();
    let hooks: serde_json::Value = serde_json::from_str(CLAUDE_CODE.hooks).unwrap();
    let group = hooks["hooks"]["SessionStart"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group.to_string().contains("--session-id-from-hook"))
        .expect("a SessionStart hook reports the session's id");
    assert!(
        group.get("matcher").is_none(),
        "every source a session starts from, resume among them"
    );
    let reporting = group["hooks"][0]["command"].as_str().unwrap().to_string();
    for shell in shells() {
        let _ = std::fs::remove_file(&arguments);
        let mut hook = Command::new(shell)
            .args(["-c", &reporting])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("MUSTER_DAEMON", &daemon)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        hook.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let output = hook.wait_with_output().unwrap();
        assert!(output.status.success(), "a hook never fails a session");
        assert!(output.stdout.is_empty(), "{shell}: what it prints reaches the model");
        let said = std::fs::read_to_string(&arguments).unwrap_or_default();
        assert_eq!(said, "[report][--agent][claude][--session-id-from-hook]", "in {shell}");
        assert_eq!(std::fs::read_to_string(&read).unwrap(), input, "the hook's input reaches it");
    }
}

const CODEX_MESSAGING: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/codex/messaging-hooks.json"));

/// Codex hands the model a hook's `additionalContext`, where a `PostToolUse` hook's exit 2 would
/// replace the tool's result (docs/observations/codex-0.154.0.md, section 7): what its messaging
/// hooks fetch reaches it that way, and nothing at all when nothing is unread.
#[test]
fn codexs_messaging_hooks_hand_what_arrived_to_the_model_as_context() {
    let scratch = Scratch::new("codex-messaging");
    let hooks: serde_json::Value = serde_json::from_str(CODEX_MESSAGING).unwrap();
    let unread = scratch.0.join("unread");
    let muster = scratch.0.join("muster");
    std::fs::write(&muster, format!("#!/bin/sh\ncat '{}' 2>/dev/null\n", unread.display()))
        .unwrap();
    std::fs::set_permissions(&muster, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run = |command: &str| {
        let ran = Command::new("/bin/sh")
            .args(["-c", command])
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", scratch.0.display()))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(ran.status.success(), "a hook never fails a session: {command}");
        String::from_utf8(ran.stdout).unwrap()
    };
    for event in ["UserPromptSubmit", "PostToolUse"] {
        let command = hooks["hooks"][event][0]["hooks"][0]["command"].as_str().unwrap();
        let _ = std::fs::remove_file(&unread);
        assert_eq!(run(command), "", "{event}: something said with nothing unread");
        std::fs::write(&unread, "#4 director: the schema changed\n").unwrap();
        let said: serde_json::Value = serde_json::from_str(&run(command)).unwrap();
        assert_eq!(said["hookSpecificOutput"]["hookEventName"], event);
        assert_eq!(
            said["hookSpecificOutput"]["additionalContext"],
            "#4 director: the schema changed"
        );
    }
}

/// OpenCode's adapter is a plugin, JavaScript OpenCode loads into itself, so it is run here under
/// `node` rather than a shell, fed the events OpenCode 1.18.34 was recorded publishing.
const OPENCODE_PLUGIN: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/opencode/plugin/muster.js");

/// Loads the plugin as OpenCode does, with a client that knows one model's context window, and
/// hands it each event in `events`, a JSON line each, waiting for each to be handled.
const OPENCODE_DRIVER: &str = r#"
import { readFileSync } from "node:fs";
const { Muster } = await import(process.argv[2]);
const models = { "big-pickle": { limit: { context: 200000 } } };
const client = { config: { providers: async () => ({ data: { providers: [{ id: "opencode", models }] } }) } };
const plugin = await Muster({ client });
for (const line of readFileSync(process.argv[3], "utf8").split("\n").filter(Boolean)) {
  await plugin.event?.({ event: JSON.parse(line) });
}
"#;

/// The events OpenCode 1.18.34 was recorded publishing, a JSON line each.
const TURNS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/opencode-1.18.34/plugin-events-turns.jsonl"
));
const SUBAGENT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/opencode-1.18.34/plugin-events-subagent.jsonl"
));

/// What OpenCode's plugin reports for `events`, a JSON line each, a line of bracketed arguments
/// per report, with `$MUSTER_DAEMON` set or not. `name` keeps each test's run apart.
fn opencode_reports(name: &str, events: &str, in_a_pane: bool) -> Vec<String> {
    // A folder per run: the tests run at once, and each counts its own reports.
    let scratch = Scratch::new(&format!("opencode-{name}-{in_a_pane}"));
    let said = scratch.0.join("said");
    let daemon = scratch.0.join("daemon");
    std::fs::write(
        &daemon,
        format!("#!/bin/sh\nprintf '[%s]' \"$@\" >> '{0}'\necho >> '{0}'\n", said.display()),
    )
    .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
    let driver = scratch.0.join("driver.mjs");
    std::fs::write(&driver, OPENCODE_DRIVER).unwrap();
    // As `.mjs`: OpenCode loads the `.js` as a module, and a node before 22 does so only by the
    // file's name, as Debian's 18 in the Linux suite's container does.
    let plugin = scratch.0.join("muster.mjs");
    std::fs::copy(OPENCODE_PLUGIN, &plugin).unwrap();
    let events_file = scratch.0.join("events.jsonl");
    std::fs::write(&events_file, events).unwrap();
    let mut node = Command::new("node");
    node.arg(&driver).arg(&plugin).arg(&events_file).env_remove("MUSTER_DAEMON");
    if in_a_pane {
        node.env("MUSTER_DAEMON", &daemon);
    }
    let ran = node.output().unwrap_or_else(|error| {
        panic!(
            "node could not be run: {error}.\n  Impact: OpenCode's plugin is untested.\n  Fix: \
             install Node.js; the Linux suite's container has it (tools/linux-run/Dockerfile)."
        )
    });
    assert!(ran.status.success(), "the plugin failed: {}", String::from_utf8_lossy(&ran.stderr));
    std::fs::read_to_string(&said).unwrap_or_default().lines().map(str::to_string).collect()
}

/// A turn, a refused permission and a turn ended with Esc, as OpenCode published them: each turn
/// reads working and ends idle, the permission prompt reads blocked until it is answered, and
/// each assistant message reports the context it used against the model's window.
#[test]
fn opencodes_plugin_reports_what_each_recorded_event_means() {
    assert_eq!(
        opencode_reports("turns", TURNS, true),
        [
            "[report][--clear]",
            "[report][--agent][opencode][--session-id][ses_efa44b0b5ffejqYaVzzLoccB84]",
            "[report][--agent][opencode][--state][working]",
            "[report][--context-used][9.77][--model][opencode/big-pickle][--cost-usd][0.0000]",
            "[report][--agent][opencode][--state][idle]",
            "[report][--agent][opencode][--state][working]",
            "[report][--agent][opencode][--state][blocked]",
            "[report][--agent][opencode][--state][working]",
            "[report][--context-used][9.80][--model][opencode/big-pickle][--cost-usd][0.0000]",
            "[report][--agent][opencode][--state][idle]",
            "[report][--agent][opencode][--state][working]",
            "[report][--agent][opencode][--state][idle]",
        ]
    );
}

/// A sub-agent runs in a session of its own, which starts and goes idle inside the main
/// session's turn: reported, it would read the pane idle while its agent works. Only what it
/// spends counts.
#[test]
fn opencodes_plugin_leaves_a_sub_agents_session_out() {
    assert_eq!(
        opencode_reports("subagent", SUBAGENT, true),
        [
            "[report][--clear]",
            "[report][--agent][opencode][--session-id][ses_efa286cb7ffexqbZM4JCqf5tgd]",
            "[report][--agent][opencode][--state][working]",
            "[report][--cost-usd][0.0000]",
            "[report][--context-used][9.28][--model][opencode/big-pickle][--cost-usd][0.0000]",
            "[report][--agent][opencode][--state][idle]",
        ]
    );
}

/// Outside a Muster pane there is no daemon to tell, and the plugin hooks nothing.
#[test]
fn opencodes_plugin_does_nothing_outside_a_pane() {
    assert_eq!(opencode_reports("outside", TURNS, false), Vec::<String>::new());
}

/// A sub-agent's permission prompt holds the whole session up as the main session's does, and what
/// a sub-agent spends is the session's spend: those of its events count. Authored, in the shapes the
/// recordings show, since none recorded a sub-agent asking.
#[test]
fn opencodes_plugin_counts_a_sub_agents_permission_prompt_and_spend() {
    let events = [
        r#"{"type":"session.created","properties":{"sessionID":"ses_main","info":{"id":"ses_main"}}}"#,
        r#"{"type":"session.status","properties":{"sessionID":"ses_main","status":{"type":"busy"}}}"#,
        r#"{"type":"session.created","properties":{"sessionID":"ses_child","info":{"id":"ses_child","parentID":"ses_main"}}}"#,
        r#"{"type":"permission.asked","properties":{"id":"per_1","sessionID":"ses_child"}}"#,
        r#"{"type":"permission.replied","properties":{"sessionID":"ses_child","requestID":"per_1","reply":"once"}}"#,
        r#"{"type":"message.updated","properties":{"info":{"id":"msg_c","sessionID":"ses_child","role":"assistant","tokens":{"total":9000},"cost":0.25,"modelID":"big-pickle","providerID":"opencode"}}}"#,
        r#"{"type":"session.idle","properties":{"sessionID":"ses_child"}}"#,
        r#"{"type":"message.updated","properties":{"info":{"id":"msg_m","sessionID":"ses_main","role":"assistant","tokens":{"total":20000},"cost":0.5,"modelID":"big-pickle","providerID":"opencode"}}}"#,
        r#"{"type":"session.idle","properties":{"sessionID":"ses_main"}}"#,
    ]
    .join("\n");
    assert_eq!(
        opencode_reports("subagent-asks", &events, true),
        [
            "[report][--clear]",
            "[report][--agent][opencode][--session-id][ses_main]",
            "[report][--agent][opencode][--state][working]",
            "[report][--agent][opencode][--state][blocked]",
            "[report][--agent][opencode][--state][working]",
            "[report][--cost-usd][0.2500]",
            "[report][--context-used][10.00][--model][opencode/big-pickle][--cost-usd][0.7500]",
            "[report][--agent][opencode][--state][idle]",
        ]
    );
}

/// Switching to an earlier session publishes no `session.created`; the first event of another
/// top-level session says the session changed, so its id is reported and the last one's spend is
/// not carried over.
#[test]
fn opencodes_plugin_follows_a_switch_to_another_session() {
    let events = [
        r#"{"type":"session.created","properties":{"sessionID":"ses_a","info":{"id":"ses_a"}}}"#,
        r#"{"type":"message.updated","properties":{"info":{"id":"msg_a","sessionID":"ses_a","role":"assistant","tokens":{"total":20000},"cost":0.5,"modelID":"big-pickle","providerID":"opencode"}}}"#,
        r#"{"type":"session.status","properties":{"sessionID":"ses_b","status":{"type":"busy"}}}"#,
        r#"{"type":"message.updated","properties":{"info":{"id":"msg_b","sessionID":"ses_b","role":"assistant","tokens":{"total":20000},"cost":0.25,"modelID":"big-pickle","providerID":"opencode"}}}"#,
    ]
    .join("\n");
    assert_eq!(
        opencode_reports("switch", &events, true),
        [
            "[report][--clear]",
            "[report][--agent][opencode][--session-id][ses_a]",
            "[report][--context-used][10.00][--model][opencode/big-pickle][--cost-usd][0.5000]",
            "[report][--agent][opencode][--session-id][ses_b]",
            "[report][--agent][opencode][--state][working]",
            "[report][--context-used][10.00][--model][opencode/big-pickle][--cost-usd][0.2500]",
        ]
    );
}
