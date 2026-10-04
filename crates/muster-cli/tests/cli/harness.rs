//! `muster harness install`, against harnesses that are scripts recording what they were asked:
//! a test never runs a real harness's install, which would write into the config of whoever runs
//! the suite.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const EXTRAS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras");

/// A home with Muster's adapters linked into it, as an install on an SSH machine leaves one, and
/// a `PATH` holding only the harnesses a test gives it.
struct Home(PathBuf);

impl Home {
    fn new(test: &str) -> Home {
        let root =
            std::env::temp_dir().join(format!("muster-harness-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home/.muster")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::os::unix::fs::symlink(EXTRAS, root.join("home/.muster/extras")).unwrap();
        Home(root)
    }

    /// A harness that writes its arguments to `<name>.ran`, a line per run, and exits `status`.
    fn harness(&self, name: &str, status: i32) {
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\necho \"{name} said $1\"\nexit {status}\n",
            self.ran(name).display()
        );
        let path = self.0.join("bin").join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn ran(&self, name: &str) -> PathBuf {
        self.0.join(format!("{name}.ran"))
    }

    fn asked(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.ran(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn extras(&self) -> PathBuf {
        self.0.join("home/.muster/extras")
    }

    fn muster(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_muster"))
            .args(arguments)
            .env_clear()
            .env("HOME", self.0.join("home"))
            .env("PATH", self.0.join("bin"))
            .stdin(Stdio::null())
            .output()
            .expect("the muster binary runs")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn claude_code_is_installed_through_its_own_plugin_commands() {
    let home = Home::new("claude");
    home.harness("claude", 0);
    let ran = home.muster(&["harness", "install", "claude-code"]);
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    let extras = home.extras();
    assert_eq!(
        home.asked("claude"),
        [
            format!("plugin marketplace add {}", extras.display()),
            "plugin install muster@muster".to_string()
        ]
    );
    let said = text(&ran.stdout);
    assert!(said.contains("claude said plugin"), "the harness's own output is passed on: {said}");
    assert!(
        said.contains(&extras.join("claude-code/statusline.sh").display().to_string()),
        "the statusline, which no command sets, is left to the person: {said}"
    );
}

#[test]
fn a_dry_run_prints_what_would_run_and_runs_nothing() {
    let home = Home::new("dry");
    home.harness("codex", 0);
    let ran = home.muster(&["harness", "install", "codex", "--dry-run"]);
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert!(home.asked("codex").is_empty(), "a dry run ran {:?}", home.asked("codex"));
    let said = text(&ran.stdout);
    let commands: Vec<&str> = said.lines().filter(|line| !line.starts_with('#')).collect();
    assert_eq!(
        commands,
        [
            format!("codex plugin marketplace add {}", home.extras().display()),
            "codex plugin add muster-codex@muster".to_string()
        ],
        "every line that is not a comment is a command, so the answer pastes into a shell: {said}"
    );
}

#[test]
fn a_command_that_fails_stops_the_install_and_says_what_is_left() {
    let home = Home::new("fails");
    home.harness("codex", 1);
    let ran = home.muster(&["harness", "install", "codex"]);
    assert_eq!(ran.status.code(), Some(1));
    assert_eq!(home.asked("codex").len(), 1, "nothing ran after the failure");
    let said = text(&ran.stderr);
    assert!(said.contains("codex plugin marketplace remove muster"), "{said}");
    assert!(said.contains("codex plugin add muster-codex@muster"), "what is left: {said}");
}

#[test]
fn a_harness_that_is_not_here_is_named_with_what_to_run() {
    let home = Home::new("missing");
    let ran = home.muster(&["harness", "install", "claude"]);
    assert_eq!(ran.status.code(), Some(1));
    let said = text(&ran.stderr);
    assert!(said.contains("`claude` could not be run"), "{said}");
    assert!(said.contains("claude plugin install muster@muster"), "{said}");
}

#[test]
fn opencode_runs_nothing_and_says_what_to_link() {
    let home = Home::new("opencode");
    home.harness("opencode", 0);
    let ran = home.muster(&["harness", "install", "opencode"]);
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert!(home.asked("opencode").is_empty());
    let said = text(&ran.stdout);
    let plugin = home.extras().join("opencode/plugin/muster.js");
    assert!(said.contains(&format!("ln -sf {}", plugin.display())), "{said}");
}

#[test]
fn a_harness_without_an_adapter_or_a_muster_without_adapters_is_refused() {
    let home = Home::new("refused");
    let ran = home.muster(&["harness", "install", "gemini"]);
    assert_eq!(ran.status.code(), Some(1));
    assert!(text(&ran.stderr).contains("claude-code, codex, opencode"), "{}", text(&ran.stderr));

    std::fs::remove_file(home.extras()).unwrap();
    let ran = home.muster(&["harness", "install", "codex", "--dry-run"]);
    assert_eq!(ran.status.code(), Some(1));
    assert!(text(&ran.stderr).contains("has no adapters beside it"), "{}", text(&ran.stderr));
}

#[test]
fn the_json_answer_lists_the_commands_and_whether_they_ran() {
    let home = Home::new("json");
    let ran = home.muster(&["--json", "harness", "install", "claude-code", "--dry-run"]);
    let answer: serde_json::Value = serde_json::from_slice(&ran.stdout).expect("JSON");
    assert_eq!(answer["ran"], false);
    assert_eq!(answer["commands"][1], "claude plugin install muster@muster");
    assert_eq!(answer["extras"], Path::new(&home.extras()).display().to_string());
}

/// The help names every harness there is an adapter for, though clap's text is written by hand.
#[test]
fn the_help_names_every_harness_with_an_adapter() {
    let home = Home::new("help");
    let help = text(&home.muster(&["harness", "install", "--help"]).stdout);
    for harness in muster_core::harnesses::WITH_ADAPTERS {
        assert!(help.contains(harness.dir), "the help leaves out {}: {help}", harness.dir);
    }
}
