//! The layering, ported in spirit from herdr v0.8.0's loader tests in
//! `src/detect/manifest/tests.rs` (Apache-2.0): herdr's remote cache is the app's layer here,
//! and its override directory is the same idea under Muster's name.

use std::path::PathBuf;

use super::*;
use crate::State;

fn codex(version: &str, state: &str, contains: &str) -> String {
    format!(
        "id = \"codex\"\nversion = \"{version}\"\nmin_engine_version = 1\n\n\
         [[rules]]\nid = \"test\"\nstate = \"{state}\"\ncontains = [\"{contains}\"]\n"
    )
}

fn detect(manifests: &Manifests, agent: &str, screen: &str) -> Detection {
    manifests.detect(Some(&Agent::new(agent)), Input { screen, ..Input::default() })
}

/// A directory of overrides for one test, gone when the test is.
struct Overrides(PathBuf);

impl Overrides {
    fn new(name: &str, files: &[(&str, &str)]) -> Overrides {
        let dir = std::env::temp_dir()
            .join(format!("muster-detect-overrides-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (file, text) in files {
            std::fs::write(dir.join(file), text).unwrap();
        }
        Overrides(dir)
    }
}

impl Drop for Overrides {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn all_built_in_manifests_parse_and_validate() {
    let manifests = Manifests::built_in();
    assert_eq!(manifests.agents().count(), BUILT_IN.len());
    for (file, text) in BUILT_IN {
        let manifest = Manifest::parse(text).unwrap_or_else(|error| panic!("{file}: {error}"));
        assert!(
            Manifest::required_engine(text).is_some_and(|engine| engine <= ENGINE_VERSION),
            "{file} needs an engine this crate does not have"
        );
        assert!(manifest.version().is_some(), "{file} has no version");
    }
}

#[test]
fn every_name_herdr_knew_an_agent_by_still_reaches_it() {
    // herdr's lookup_agent table (src/detect/mod.rs), minus omp and mastracode, which had no
    // screen manifest and so are not agents here.
    let manifests = Manifests::built_in();
    for (agent, names) in [
        ("pi", &["pi"][..]),
        ("claude", &["claude", "claude-code"]),
        ("codex", &["codex"]),
        ("gemini", &["gemini"]),
        ("cursor", &["cursor", "cursor-agent"]),
        ("devin", &["devin", "devin-cli", "devin cli"]),
        ("agy", &["agy", "antigravity", "antigravity-cli"]),
        ("cline", &["cline"]),
        ("opencode", &["opencode", "opencode2", "open-code"]),
        ("copilot", &["copilot", "github-copilot", "ghcs"]),
        ("kimi", &["kimi", "kimi-code", "kimi code"]),
        ("kiro", &["kiro", "kiro-cli"]),
        ("droid", &["droid"]),
        ("amp", &["amp", "amp-local"]),
        ("grok", &["grok", "grok-build"]),
        ("hermes", &["hermes", "hermes-agent"]),
        ("kilo", &["kilo", "kilo-code", "kilo code"]),
        ("qodercli", &["qodercli", "qoderclicn", "qoder", "qodercn"]),
        ("maki", &["maki"]),
    ] {
        for name in names {
            assert_eq!(
                manifests.agent_named(name).as_ref().map(Agent::id),
                Some(agent),
                "{name} should mean {agent}"
            );
        }
    }
    assert_eq!(manifests.agent_named(" Claude.EXE ").as_ref().map(Agent::id), Some("claude"));
    for name in ["omp", "mastracode", "bash", "claude-codex", ""] {
        assert_eq!(manifests.agent_named(name), None, "{name} is no agent");
    }
}

#[test]
fn script_paths_find_an_agent_inside_its_package() {
    let manifests = Manifests::built_in();
    let pi = |path: &str| manifests.agent_for_script_path(path).map(|agent| agent.id().to_string());
    assert_eq!(
        pi("/usr/lib/node_modules/@earendil-works/pi-coding-agent/dist/cli.js").as_deref(),
        Some("pi")
    );
    assert_eq!(pi("/usr/lib/node_modules/@earendil-works/pi-coding-agent/dist/other.js"), None);
}

#[test]
fn an_app_manifest_replaces_the_built_in_one() {
    let (manifests, warnings) = Manifests::load(
        &[("codex.toml".into(), codex("9999.01.01.1", "blocked", "app-ready"))],
        None,
    );
    assert_eq!(warnings, []);
    assert_eq!(detect(&manifests, "codex", "app-ready").state, State::Blocked);
    assert_eq!(
        manifests.source(&Agent::new("codex")),
        Some(&Source::App("codex.toml".to_string()))
    );
}

#[test]
fn an_older_app_manifest_does_not_shadow_a_newer_built_in_one() {
    let (manifests, warnings) = Manifests::load(
        &[("codex.toml".into(), codex("2026.06.10.0", "blocked", "app-ready"))],
        None,
    );
    assert_eq!(detect(&manifests, "codex", "app-ready").state, State::Idle);
    assert_eq!(manifests.source(&Agent::new("codex")), Some(&Source::BuiltIn));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].problem.contains("older than"), "{}", warnings[0]);
}

#[test]
fn an_app_manifest_must_be_versioned_and_gated() {
    let unversioned =
        "id = \"codex\"\nmin_engine_version = 1\n[[rules]]\nid = \"t\"\ncontains = [\"x\"]\n";
    let ungated =
        "id = \"codex\"\nversion = \"9999.1\"\n[[rules]]\nid = \"t\"\ncontains = [\"x\"]\n";
    let (_, warnings) = Manifests::load(
        &[("a.toml".into(), unversioned.into()), ("b.toml".into(), ungated.into())],
        None,
    );
    assert_eq!(warnings.len(), 2);
    assert!(warnings[0].problem.contains("no version"));
    assert!(warnings[1].problem.contains("no min_engine_version"));
}

#[test]
fn a_manifest_for_a_newer_engine_is_refused_for_that_rather_than_its_new_keys() {
    let newer = "id = \"codex\"\nversion = \"9999.1\"\nmin_engine_version = 99\nnew_key = 1\n\
                 [[rules]]\nid = \"t\"\ncontains = [\"x\"]\n";
    let (manifests, warnings) = Manifests::load(&[("codex.toml".into(), newer.into())], None);
    assert_eq!(manifests.source(&Agent::new("codex")), Some(&Source::BuiltIn));
    assert!(warnings[0].problem.contains("needs detection engine 99"), "{}", warnings[0]);
}

#[test]
fn a_local_override_shadows_the_app_manifest() {
    let overrides =
        Overrides::new("shadows", &[("codex.toml", &codex("1", "idle", "local-ready"))]);
    let (manifests, warnings) = Manifests::load(
        &[("codex.toml".into(), codex("9999.01.01.1", "blocked", "app-ready"))],
        Some(&overrides.0),
    );
    assert_eq!(warnings, []);
    assert_eq!(detect(&manifests, "codex", "local-ready").rule.as_deref(), Some("test"));
    assert_eq!(detect(&manifests, "codex", "app-ready").rule, None);
    assert!(matches!(manifests.source(&Agent::new("codex")), Some(Source::Override(_))));
}

#[test]
fn an_invalid_override_falls_back_to_the_app_manifest() {
    let overrides = Overrides::new("invalid", &[("codex.toml", "id = ")]);
    let (manifests, warnings) = Manifests::load(
        &[("codex.toml".into(), codex("9999.01.01.1", "blocked", "app-ready"))],
        Some(&overrides.0),
    );
    assert_eq!(detect(&manifests, "codex", "app-ready").state, State::Blocked);
    assert_eq!(warnings.len(), 1);
    assert!(matches!(warnings[0].source, Source::Override(_)));
}

#[test]
fn an_override_named_for_another_agent_is_refused() {
    let overrides = Overrides::new("misnamed", &[("claude.toml", &codex("1", "blocked", "x"))]);
    let (manifests, warnings) = Manifests::load(&[], Some(&overrides.0));
    assert_eq!(manifests.source(&Agent::new("codex")), Some(&Source::BuiltIn));
    assert_eq!(manifests.source(&Agent::new("claude")), Some(&Source::BuiltIn));
    assert!(warnings[0].problem.contains("name it codex.toml"), "{}", warnings[0]);
}

#[test]
fn an_override_can_add_an_agent_herdr_never_knew() {
    let manifest = "id = \"mine\"\naliases = [\"my-agent\"]\n\
                    [[rules]]\nid = \"busy\"\nstate = \"working\"\ncontains = [\"thinking\"]\n";
    let overrides =
        Overrides::new("new-agent", &[("my-agent.toml", manifest), ("notes.txt", "ignored")]);
    let (manifests, warnings) = Manifests::load(&[], Some(&overrides.0));
    assert_eq!(warnings, []);
    assert_eq!(manifests.agent_named("my-agent"), Some(Agent::new("mine")));
    assert_eq!(detect(&manifests, "mine", "thinking...").state, State::Working);
}

#[test]
fn a_missing_override_directory_is_no_overrides() {
    let (manifests, warnings) =
        Manifests::load(&[], Some(Path::new("/nonexistent/muster-detect/agent-detection")));
    assert_eq!(warnings, []);
    assert_eq!(manifests.agents().count(), BUILT_IN.len());
}

#[test]
fn no_agent_is_unknown_and_a_vanished_one_is_idle() {
    let manifests = Manifests::built_in();
    assert_eq!(manifests.detect(None, Input::default()).state, State::Unknown);
    assert_eq!(detect(&manifests, "gone", "anything").state, State::Idle);
}
