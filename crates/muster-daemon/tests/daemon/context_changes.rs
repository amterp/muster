//! An agent's context cleared or compacted reaches the API on its pane (`Pane.context_change`):
//! from what a harness's hooks report, and, here, from what those hooks were recorded being handed
//! (docs/observations/claude-code-2.1.319.md, docs/observations/codex-0.166.0.md), played through
//! the hooks `extras/` ships and the real `muster-daemon report`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::claude_code::Scratch;
use crate::persistence::record;
use crate::support::*;
use api::event::Event;
use muster_core::diagnostics::clock::format_iso8601;
use proto::ContextChangeKind::{self, Cleared, Compacted};
use proto::pane_request;

const CLAUDE_HOOKS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/claude-code/hooks/hooks.json"));
const CODEX_HOOKS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/codex/hooks/hooks.json"));

const CLAUDE_ASKED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/claude-code-2.1.319/context-hooks-asked.jsonl"
));
const CLAUDE_AUTOMATIC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/claude-code-2.1.319/context-hooks-automatic.jsonl"
));
const CLAUDE_BOUNDARIES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/claude-code-2.1.319/compact-boundary.jsonl"
));
const CODEX_ASKED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/codex-0.166.0/context-hooks-asked.jsonl"
));
const CODEX_FORK: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/codex-0.166.0/context-hooks-fork.jsonl"
));
const CODEX_AUTOMATIC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/codex-0.166.0/context-hooks-automatic.jsonl"
));

fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is past 1970");
    u64::try_from(since.as_millis()).expect("milliseconds fit")
}

/// The commands the file gives `event` for `row`, a group's matcher read as the harness reads it
/// on a `source` or `trigger`.
fn commands(hooks: &serde_json::Value, event: &str, row: &serde_json::Value) -> Vec<String> {
    let groups = hooks["hooks"][event].as_array().cloned().unwrap_or_default();
    let on = row["source"].as_str().or(row["trigger"].as_str()).unwrap_or_default();
    groups
        .iter()
        .filter(|group| {
            group["matcher"].as_str().is_none_or(|matcher| matcher.split('|').any(|m| m == on))
        })
        .flat_map(|group| group["hooks"].as_array().unwrap().iter())
        .map(|hook| hook["command"].as_str().unwrap().to_string())
        .collect()
}

/// A recorded Claude Code compaction left its boundary in the session's own transcript.
fn own_boundary(row: &serde_json::Value) -> Option<serde_json::Value> {
    let hook_ran = row["recorded_at"].as_str()?;
    CLAUDE_BOUNDARIES
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|boundary| {
            let at = boundary["timestamp"].as_str().unwrap();
            boundary["sessionId"] == row["session_id"] && at[..19] >= hook_ran[..19] && {
                let (ran, at) = (seconds(hook_ran), seconds(at));
                at - ran <= 1
            }
        })
}

fn seconds(stamp: &str) -> i64 {
    let (minute, second) = (&stamp[14..16], &stamp[17..19]);
    minute.parse::<i64>().unwrap() * 60 + second.parse::<i64>().unwrap()
}

/// Plays `recorded` through `hooks` at the fake agent in a fresh daemon's pane, and answers each
/// change its pane's record took on the way. The fake agent is detected as `claude`, so the other
/// harness's hooks report under that name.
fn played(hooks: &str, reporting_as: &str, recorded: &str) -> Vec<ContextChangeKind> {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let scratch = Scratch::new(&format!("context-changes-{}", recorded.len()));
    let transcript = scratch.0.join("transcript.jsonl");
    std::fs::write(&transcript, "").unwrap();
    let hooks: serde_json::Value = serde_json::from_str(hooks).unwrap();
    let mut seen = Vec::new();
    let mut last = None;
    for line in recorded.lines() {
        let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
        let Some(event) = row["hook_event_name"].as_str().map(str::to_string) else { continue };
        // Each compaction gets a transcript of its own: played this fast, one's boundary would
        // be recent enough to confirm the next.
        if event == "PostCompact" {
            let boundary = own_boundary(&row).map(|mut boundary| {
                boundary["timestamp"] = format_iso8601(now_ms().cast_signed()).into();
                format!("{boundary}\n")
            });
            std::fs::write(&transcript, boundary.unwrap_or_default()).unwrap();
        }
        row["transcript_path"] = transcript.display().to_string().into();
        for command in commands(&hooks, &event, &row) {
            run(&command.replace(reporting_as, "--agent claude"), &row, &daemon);
        }
        let now = record(&snapshot(&mut control), "p1").context_change;
        if let Some(change) = now.filter(|change| Some(change.at_ms) != last) {
            last = Some(change.at_ms);
            seen.push(change.kind());
        }
    }
    seen
}

fn run(command: &str, input: &serde_json::Value, daemon: &Daemon) {
    let mut hook = Command::new("/bin/sh")
        .args(["-c", command])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("MUSTER_DAEMON", muster_harness::built_daemon())
        .env("MUSTER_DAEMON_SOCKET", socket(daemon))
        .env("MUSTER_PANE", "p1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = hook.stdin.take().unwrap();
    // A hook that ignores its input closes the pipe first.
    let _ = stdin.write_all(input.to_string().as_bytes());
    drop(stdin);
    assert!(hook.wait().unwrap().success(), "a hook never fails a session: {command}");
}

fn socket(daemon: &Daemon) -> &Path {
    daemon.socket_path()
}

/// `/compact`, then `/clear`. A compaction with nothing to compact and one cancelled with Esc run
/// `PreCompact` and go no further, so neither is a change.
#[test]
fn claude_codes_recorded_clear_and_compaction_reach_the_pane() {
    assert_eq!(played(CLAUDE_HOOKS, "--agent claude", CLAUDE_ASKED), [Compacted, Cleared]);
}

/// Three compactions fired the hooks, the middle one a sub-agent's, which its parent's transcript
/// does not record. Starting and resuming change nothing.
#[test]
fn a_sub_agents_compaction_is_not_claude_codes_own() {
    assert_eq!(played(CLAUDE_HOOKS, "--agent claude", CLAUDE_AUTOMATIC), [Compacted, Compacted]);
}

/// `/compact`, `/clear`, `/new`, and `/compact`. `/new` reads as a process starting, and only the
/// session it replaced says otherwise.
#[test]
fn codexs_recorded_clear_new_and_compaction_reach_the_pane() {
    let played = played(CODEX_HOOKS, "--agent codex", CODEX_ASKED);
    assert_eq!(played, [Compacted, Cleared, Cleared, Compacted]);
}

/// `/fork` starts another session and keeps the conversation, so it clears nothing.
#[test]
fn codexs_fork_keeps_its_context() {
    assert_eq!(played(CODEX_HOOKS, "--agent codex", CODEX_FORK), []);
}

/// Six compactions fired the hooks, one of them a sub-agent's, which says so. Cancelling one
/// with Esc and resuming change nothing.
#[test]
fn codexs_sub_agents_compaction_is_not_its_own() {
    assert_eq!(played(CODEX_HOOKS, "--agent codex", CODEX_AUTOMATIC), [Compacted; 5]);
}

fn agent_in_p1() -> (Daemon, Control) {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    (daemon, control)
}

fn report(kind: ContextChangeKind) -> pane_request::Report {
    let mut report = pane_request::Report {
        pane: "p1".to_string(),
        agent: "claude".to_string(),
        ..Default::default()
    };
    report.set_context_change(kind);
    report
}

fn said(control: &mut Control, report: pane_request::Report) {
    expect(control, pane(pane_request::Request::Report(report)), proto::Outcome::Done);
}

/// A subscriber hears the change as it happens, with the time, and one that comes later reads the
/// same change in its snapshot, which is how a client that was away catches up.
#[test]
fn a_context_change_reaches_a_subscriber_and_a_later_snapshot() {
    let (daemon, mut control) = agent_in_p1();
    let mut api = daemon.api();
    let snapshot = api.subscribe(&[api::Topic::Panes]).snapshot().clone();
    assert_eq!(snapshot.panes[0].context_change, None, "nothing has happened yet");

    let before = now_ms();
    said(&mut control, report(Cleared));
    let after = now_ms();

    let event = api.event_matching("p1 to say its context was cleared", |event| {
        matches!(&event.event, Some(Event::PaneChanged(changed))
            if changed.pane.as_ref().is_some_and(|pane| pane.context_change.is_some()))
    });
    let Some(Event::PaneChanged(changed)) = event.event else { unreachable!() };
    let heard = changed.pane.unwrap().context_change.unwrap();
    assert_eq!(heard.kind(), api::ContextChangeKind::Cleared);
    assert!((before..=after).contains(&heard.at_ms), "{before} to {after}, and {}", heard.at_ms);

    let mut later = daemon.api();
    let snapshot = later.subscribe(&[api::Topic::Panes]).snapshot().clone();
    assert_eq!(snapshot.panes[0].context_change, Some(heard));
}

/// A harness that clears its facts and says its context changed may do so in either order.
#[test]
fn forgetting_an_agents_facts_leaves_its_context_change() {
    let (_daemon, mut control) = agent_in_p1();
    let with_a_fact = pane_request::Report { model: Some("Opus".to_string()), ..report(Compacted) };
    said(&mut control, with_a_fact);
    let clear = pane_request::Report { clear: true, ..report(ContextChangeKind::None) };
    said(&mut control, clear);
    let held = record(&snapshot(&mut control), "p1").context_change;
    assert_eq!(held.map(|change| change.kind()), Some(Compacted));
}

/// A sub-agent's hook can still be waiting on the transcript when its parent compacts, and then
/// finds the parent's boundary too: both report it, and it is one compaction.
#[test]
fn two_reports_of_one_compaction_are_one_change() {
    let (_daemon, mut control) = agent_in_p1();
    let named = |id: &str| pane_request::Report {
        context_change_id: Some(id.to_string()),
        ..report(Compacted)
    };
    let change = |control: &mut Control| record(&snapshot(control), "p1").context_change;

    said(&mut control, named("boundary-1"));
    let first = change(&mut control);
    expect(
        &mut control,
        pane(pane_request::Request::Report(named("boundary-1"))),
        proto::Outcome::AlreadySo,
    );
    assert_eq!(change(&mut control), first, "the same boundary again");

    std::thread::sleep(std::time::Duration::from_millis(2));
    said(&mut control, named("boundary-2"));
    assert_ne!(change(&mut control), first, "another compaction");
}

/// As recorded, a sub-agent's `PostCompact` was still within its wait when its parent's began, with
/// one transcript between them, and the shipped hook has to come to one change.
#[test]
fn a_hook_left_waiting_shares_its_parents_compaction() {
    let (daemon, mut control) = agent_in_p1();
    let scratch = Scratch::new("context-changes-overlap");
    let transcript = scratch.0.join("transcript.jsonl");
    std::fs::write(&transcript, "").unwrap();
    let hooks: serde_json::Value = serde_json::from_str(CLAUDE_HOOKS).unwrap();
    let input = serde_json::json!({ "transcript_path": transcript.display().to_string() });
    let command = commands(&hooks, "PostCompact", &input).remove(0);

    std::thread::scope(|threads| {
        threads.spawn(|| run(&command, &input, &daemon));
        std::thread::sleep(std::time::Duration::from_millis(500));
        threads.spawn(|| run(&command, &input, &daemon));
        let line = CLAUDE_BOUNDARIES.lines().last().unwrap();
        let mut boundary: serde_json::Value = serde_json::from_str(line).unwrap();
        boundary["timestamp"] = format_iso8601(now_ms().cast_signed()).into();
        std::fs::write(&transcript, format!("{boundary}\n")).unwrap();
    });

    let held = record(&snapshot(&mut control), "p1").context_change;
    assert_eq!(held.map(|change| change.kind()), Some(Compacted));
    let log = std::fs::read_to_string(daemon.root().join("daemon.log")).unwrap_or_default();
    assert_eq!(log.matches("session.context.changed").count(), 1, "{log}");
}

/// A session's start is a clear only when the agent went on running from another session.
#[test]
fn a_session_started_clears_only_a_session_the_agent_still_held() {
    let (_daemon, mut control) = agent_in_p1();
    let started = |id: &str| pane_request::Report {
        session_id: Some(id.to_string()),
        ..report(ContextChangeKind::SessionStarted)
    };
    let change = |control: &mut Control| record(&snapshot(control), "p1").context_change;

    said(&mut control, started("s-1"));
    assert_eq!(change(&mut control), None, "the agent's first session replaced nothing");

    expect(
        &mut control,
        pane(pane_request::Request::Report(started("s-1"))),
        proto::Outcome::AlreadySo,
    );
    assert_eq!(change(&mut control), None, "the same session again replaced nothing");

    said(&mut control, started("s-2"));
    assert_eq!(change(&mut control).map(|change| change.kind()), Some(Cleared));
}

#[test]
fn a_change_from_another_agent_or_none_is_not_the_panes() {
    let (_daemon, mut control) = agent_in_p1();
    let other = pane_request::Report { agent: "codex".to_string(), ..report(Cleared) };
    expect(&mut control, pane(pane_request::Request::Report(other)), proto::Outcome::AlreadySo);
    let nameless = pane_request::Report { agent: String::new(), ..report(Cleared) };
    expect(&mut control, pane(pane_request::Request::Report(nameless)), proto::Outcome::Refused);
    assert_eq!(record(&snapshot(&mut control), "p1").context_change, None);
}
