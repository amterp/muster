//! What the shipped manifests type to compact a session and run to resume one, as Codex 0.167.0
//! and OpenCode 1.19.5 were recorded doing (`corpus/codex-0.167.0/compact-and-resume.txt`,
//! `corpus/opencode-1.19.5/compact-and-resume.txt`).

use muster_detect::{Agent, Manifests};

fn resumed(agent: &str, started_with: &[&str]) -> (String, Option<String>) {
    let arguments: Vec<String> = started_with.iter().map(ToString::to_string).collect();
    let resume = Manifests::built_in()
        .session_resume(&Agent::new(agent), "s-2", &arguments, None)
        .unwrap_or_else(|| panic!("{agent} has no resume"));
    (resume.command.join(" "), resume.uncarried)
}

/// Codex's `/compact` takes no focus: words after it go to the model as a message. OpenCode's
/// needs a second Return, which a compaction typed as one line does not send.
#[test]
fn codex_compacts_with_a_bare_slash_command_and_opencode_is_not_compacted() {
    let manifests = Manifests::built_in();
    let codex = Agent::new("codex");
    assert_eq!(manifests.session_compact(&codex, None), Some(Ok("/compact".to_string())));
    let focused = manifests.session_compact(&codex, Some("keep the word pong"));
    assert!(focused.is_some_and(|line| line.is_err()), "codex refuses a focus");
    assert!(!manifests.compaction_takes_focus(&codex));
    assert!(manifests.compaction_takes_focus(&Agent::new("claude")));
    assert_eq!(manifests.session_compact(&Agent::new("opencode"), None), None);
}

/// The flags the recording started Codex with go along, so the session comes back on the same
/// model, effort and permissions: `codex resume` runs with what it is given, not with what the
/// session started with.
#[test]
fn codex_resumes_with_the_flags_it_was_started_with() {
    let started =
        ["-m", "gpt-6.1-terra", "-c", "model_reasoning_effort=\"low\"", "-s", "read-only"];
    let (command, uncarried) =
        resumed("codex", &[&started[..], &["-a", "on-request", "--no-daemon"]].concat());
    assert_eq!(
        command,
        "codex resume -m gpt-6.1-terra -c model_reasoning_effort=\"low\" -s read-only -a \
         on-request --no-daemon s-2"
    );
    assert_eq!(uncarried, None);
    // Started with a first prompt, or already resumed by Muster once: the prompt and the old id
    // stay behind.
    assert_eq!(
        resumed("codex", &["--approve-for-me", "fix the build"]).0,
        "codex resume --approve-for-me s-2"
    );
    let again = resumed("codex", &["resume", "-m", "gpt-6.1-terra", "--no-daemon", "s-1"]);
    assert_eq!(again, ("codex resume -m gpt-6.1-terra --no-daemon s-2".to_string(), None));
}

/// OpenCode resumes by `--session`, with its model and agent; an earlier `--session` or
/// `--continue` is replaced.
#[test]
fn opencode_resumes_by_its_session_flag_with_the_flags_it_was_started_with() {
    let (command, uncarried) =
        resumed("opencode", &["-m", "opencode/big-pickle", "--agent", "plan"]);
    assert_eq!(command, "opencode -m opencode/big-pickle --agent plan --session s-2");
    assert_eq!(uncarried, None);
    let again = resumed("opencode", &["-m", "opencode/big-pickle", "--session", "s-1"]);
    assert_eq!(again.0, "opencode -m opencode/big-pickle --session s-2");
    assert_eq!(resumed("opencode", &["--continue", "--prompt", "hi"]).0, "opencode --session s-2");
}
