//! What the shipped manifests type to compact a session and run to resume one, as Codex 0.167.0
//! was recorded doing (`corpus/codex-0.167.0/compact-and-resume.txt`).

use muster_detect::{Agent, Manifests};

fn resumed(agent: &str, started_with: &[&str]) -> (String, Option<String>) {
    let arguments: Vec<String> = started_with.iter().map(ToString::to_string).collect();
    let resume = Manifests::built_in()
        .session_resume(&Agent::new(agent), "s-2", &arguments, None)
        .unwrap_or_else(|| panic!("{agent} has no resume"));
    (resume.command.join(" "), resume.uncarried)
}

/// Codex's `/compact` takes no focus: words after it go to the model as a message.
#[test]
fn codex_compacts_with_a_bare_slash_command() {
    let manifests = Manifests::built_in();
    let codex = Agent::new("codex");
    assert_eq!(manifests.session_compact(&codex, None), Some(Ok("/compact".to_string())));
    let focused = manifests.session_compact(&codex, Some("keep the word pong"));
    assert!(focused.is_some_and(|line| line.is_err()), "codex refuses a focus");
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
