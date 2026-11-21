//! `muster harness install`, against harnesses that are scripts recording what they were asked:
//! a test never runs a real harness's install, which would write into the config of whoever runs
//! the suite.

use std::fmt::Write as _;
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

    /// A `codex` that writes its arguments down as [`Home::harness`]'s does, lists `listing` as
    /// its marketplaces when asked for them as JSON, and runs `features` for `features disable`.
    fn codex(&self, listing: &str, features: &str) {
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in\n\
             'plugin marketplace list --json') printf '%s\\n' '{listing}'; exit 0 ;;\n\
             'features disable daemon_auto_start') {features} ;;\n\
             esac\necho \"codex said $1\"\n",
            self.ran("codex").display()
        );
        let path = self.0.join("bin/codex");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// What `codex plugin marketplace list --json` prints with `muster` registered at `root`.
    fn listing(root: &Path) -> String {
        format!(r#"{{"marketplaces": [{{"name": "muster", "root": "{}"}}]}}"#, root.display())
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
    assert_eq!(
        home.asked("codex"),
        Vec::<String>::new(),
        "a dry run must not invoke the harness, even to query its registration"
    );
    let said = text(&ran.stdout);
    let commands: Vec<&str> = said.lines().filter(|line| !line.starts_with('#')).collect();
    assert_eq!(
        commands,
        [
            format!("codex plugin marketplace add {}", home.extras().display()),
            "codex plugin add muster-codex@muster".to_string(),
            "codex features disable daemon_auto_start".to_string()
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
    assert_eq!(
        home.asked("codex"),
        [
            "plugin marketplace list --json",
            &format!("plugin marketplace add {}", home.extras().display())
        ],
        "nothing ran after the failure"
    );
    let said = text(&ran.stderr);
    assert!(said.contains("codex plugin marketplace remove muster"), "{said}");
    assert!(said.contains("codex plugin add muster-codex@muster"), "what is left: {said}");
}

/// After an update, Codex holds `muster` at the earlier version's resolved path, and refuses to add
/// the new one beside it, so an install replaces it.
#[test]
fn codex_is_installed_again_over_an_earlier_musters_marketplace() {
    let home = Home::new("stale");
    let earlier = home.0.join("home/.muster/daemon/0.13.0/extras");
    home.codex(&Home::listing(&earlier), "exit 0");
    let ran = home.muster(&["harness", "install", "codex"]);
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert_eq!(
        home.asked("codex"),
        [
            "plugin marketplace list --json".to_string(),
            "plugin marketplace remove muster".to_string(),
            format!("plugin marketplace add {}", home.extras().display()),
            "plugin add muster-codex@muster".to_string(),
            "features disable daemon_auto_start".to_string(),
        ]
    );
    assert!(text(&ran.stdout).contains("refreshes Codex's copy"));
}

/// One added from anywhere else is somebody's choice: the add is left to fail, saying so.
#[test]
fn a_marketplace_added_from_a_checkout_is_left_alone() {
    let home = Home::new("checkout");
    let checkout = home.0.join("src/muster/extras");
    std::fs::create_dir_all(&checkout).unwrap();
    home.codex(&Home::listing(&checkout.canonicalize().unwrap()), "exit 0");
    let ran = home.muster(&["harness", "install", "codex"]);
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert!(!home.asked("codex").iter().any(|command| command.contains("remove")));
}

/// A Codex that has no `daemon_auto_start` has no shared app-server, so refusing the flag is done.
#[test]
fn a_codex_without_the_app_server_setting_is_installed_all_the_same() {
    let home = Home::new("old-codex");
    let unknown = "echo 'Error: Unknown feature flag: daemon_auto_start' >&2; exit 1";
    home.codex(r#"{"marketplaces": []}"#, unknown);
    let ran = home.muster(&["harness", "install", "codex"]);
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert_eq!(home.asked("codex").last().unwrap(), "features disable daemon_auto_start");
    assert!(text(&ran.stdout).contains("no shared app-server"), "{}", text(&ran.stdout));
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

#[test]
fn a_codex_json_dry_run_never_queries_the_harness() {
    let home = Home::new("codex-json-dry");
    home.harness("codex", 0);
    let ran = home.muster(&["--json", "harness", "install", "codex", "--dry-run"]);
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    let answer: serde_json::Value = serde_json::from_slice(&ran.stdout).expect("JSON");
    assert_eq!(answer["ran"], false);
    assert!(home.asked("codex").is_empty(), "a JSON dry run must not run Codex");
    assert_eq!(answer["commands"][1], "codex plugin add muster-codex@muster");
}

#[test]
fn the_help_names_every_harness_with_an_adapter() {
    let home = Home::new("help");
    let help = text(&home.muster(&["harness", "install", "--help"]).stdout);
    for (_, harness) in muster_adapters::catalog::adapters() {
        assert!(help.contains(harness.dir), "the help leaves out {}: {help}", harness.dir);
    }
}

impl Home {
    fn copy(&self) -> PathBuf {
        self.0.join("home/.codex/plugins/cache/muster/muster-codex/local")
    }

    fn copied_plugin(&self) {
        for name in
            [".codex-plugin/plugin.json", "hooks/hooks.json", "messaging-hooks.json", "README.md"]
        {
            let path = self.copy().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::copy(Path::new(EXTRAS).join("codex").join(name), path).unwrap();
        }
    }

    fn refresh_codex(&self, listing: &str) {
        self.codex(listing, "exit 93");
        let path = self.0.join("bin/codex");
        let mut script = std::fs::read_to_string(&path).unwrap();
        write!(
            script,
            "\nif [ \"$*\" = 'plugin add muster-codex@muster' ]; then\n\
             /bin/cp -R '{EXTRAS}/codex/.' '{}'\nfi\n",
            self.copy().display()
        )
        .unwrap();
        std::fs::write(path, script).unwrap();
    }

    fn refresh(&self) -> Output {
        self.muster(&["harness", "refresh", "codex"])
    }
}

#[test]
fn refresh_leaves_a_codex_without_a_plugin_alone() {
    let home = Home::new("refresh-no-plugin");
    home.harness("codex", 0);
    let ran = home.refresh();
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert!(home.asked("codex").is_empty());
    assert!(text(&ran.stdout).contains("not installed"), "{}", text(&ran.stdout));
}

#[test]
fn refresh_preserves_hook_trust_when_the_content_matches() {
    let home = Home::new("refresh-matches");
    home.copied_plugin();
    home.harness("codex", 0);
    std::fs::write(home.copy().join("README.md"), "an older README").unwrap();
    let ran = home.refresh();
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert!(home.asked("codex").is_empty(), "unchanged hooks must not be reinstalled");
    assert!(text(&ran.stdout).contains("matches"), "{}", text(&ran.stdout));
}

#[test]
fn refresh_updates_changed_hooks_without_changing_codex_features() {
    let home = Home::new("refresh-changed");
    home.copied_plugin();
    std::fs::write(home.copy().join("hooks/hooks.json"), "{}").unwrap();
    let earlier = home.0.join("home/.muster/daemon/0.13.0/extras");
    home.refresh_codex(&Home::listing(&earlier));
    let ran = home.refresh();
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert_eq!(
        home.asked("codex"),
        [
            "plugin marketplace list --json".to_string(),
            "plugin marketplace remove muster".to_string(),
            format!("plugin marketplace add {}", home.extras().display()),
            "plugin add muster-codex@muster".to_string(),
        ]
    );
    assert_eq!(
        std::fs::read(home.copy().join("hooks/hooks.json")).unwrap(),
        std::fs::read(Path::new(EXTRAS).join("codex/hooks/hooks.json")).unwrap()
    );
    assert!(text(&ran.stdout).contains("refreshed"), "{}", text(&ran.stdout));
}

#[test]
fn refresh_never_adds_a_marketplace_that_is_not_registered() {
    let home = Home::new("refresh-no-marketplace");
    home.copied_plugin();
    std::fs::write(home.copy().join("hooks/hooks.json"), "{}").unwrap();
    home.codex(r#"{"marketplaces": []}"#, "exit 93");
    let ran = home.refresh();
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert_eq!(home.asked("codex"), ["plugin marketplace list --json"]);
    assert!(text(&ran.stdout).contains("not registered"), "{}", text(&ran.stdout));
}

#[test]
fn refresh_does_not_replace_a_marketplace_from_a_checkout() {
    let home = Home::new("refresh-checkout");
    home.copied_plugin();
    std::fs::write(home.copy().join("hooks/hooks.json"), "{}").unwrap();
    let checkout = home.0.join("src/muster/extras");
    std::fs::create_dir_all(&checkout).unwrap();
    home.codex(&Home::listing(&checkout), "exit 93");
    let ran = home.refresh();
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert_eq!(home.asked("codex"), ["plugin marketplace list --json"]);
    assert!(text(&ran.stdout).contains("another source"), "{}", text(&ran.stdout));
}

#[test]
fn a_failed_refresh_reports_which_command_failed_and_preserves_the_copy() {
    let home = Home::new("refresh-failure");
    home.copied_plugin();
    std::fs::write(home.copy().join("hooks/hooks.json"), "{}").unwrap();
    home.harness("codex", 1);
    let ran = home.refresh();
    assert_eq!(ran.status.code(), Some(1));
    let said = text(&ran.stderr);
    assert!(said.contains("codex plugin marketplace list --json"), "{said}");
    assert!(said.contains("muster harness install codex"), "{said}");
    assert_eq!(std::fs::read_to_string(home.copy().join("hooks/hooks.json")).unwrap(), "{}");
}

#[test]
fn concurrent_refreshes_do_not_run_codex_twice() {
    use std::os::fd::AsRawFd;
    let home = Home::new("refresh-locked");
    home.copied_plugin();
    std::fs::write(home.copy().join("hooks/hooks.json"), "{}").unwrap();
    home.harness("codex", 0);
    let lock =
        std::fs::File::create(home.0.join("home/.codex/plugins/.muster-refresh.lock")).unwrap();
    // SAFETY: lock owns the open file descriptor until this test ends.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
    let ran = home.refresh();
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert!(home.asked("codex").is_empty());
    assert!(text(&ran.stdout).contains("already refreshing"), "{}", text(&ran.stdout));
}

#[test]
fn an_invalid_marketplace_listing_is_a_refresh_failure() {
    let home = Home::new("refresh-invalid-listing");
    home.copied_plugin();
    std::fs::write(home.copy().join("hooks/hooks.json"), "{}").unwrap();
    home.codex("not JSON", "exit 93");
    let ran = home.refresh();
    assert_eq!(ran.status.code(), Some(1));
    assert!(text(&ran.stderr).contains("not JSON"), "{}", text(&ran.stderr));
    assert_eq!(home.asked("codex"), ["plugin marketplace list --json"]);
}

#[test]
fn codex_success_without_a_refreshed_copy_is_reported_as_a_failure() {
    let home = Home::new("refresh-still-stale");
    home.copied_plugin();
    std::fs::write(home.copy().join("hooks/hooks.json"), "{}").unwrap();
    home.codex(&Home::listing(&home.extras().canonicalize().unwrap()), "exit 93");
    let ran = home.refresh();
    assert_eq!(ran.status.code(), Some(1));
    assert!(text(&ran.stderr).contains("still differs"), "{}", text(&ran.stderr));
    assert_eq!(home.asked("codex").last().unwrap(), "plugin add muster-codex@muster");
}

#[test]
fn automatic_refresh_uses_the_login_profiles_path_and_codex_home() {
    let home = Home::new("refresh-login");
    home.copied_plugin();
    std::fs::write(home.copy().join("hooks/hooks.json"), "{}").unwrap();
    home.refresh_codex(&Home::listing(&home.extras().canonicalize().unwrap()));
    let custom = home.0.join("custom-codex");
    std::fs::rename(home.0.join("home/.codex"), &custom).unwrap();
    let copy = custom.join("plugins/cache/muster/muster-codex/local");
    let codex = home.0.join("bin/codex");
    let script = std::fs::read_to_string(&codex)
        .unwrap()
        .replace(&home.copy().display().to_string(), &copy.display().to_string());
    std::fs::write(codex, script).unwrap();
    std::fs::write(
        home.0.join("home/.zprofile"),
        format!("export CODEX_HOME='{}'\n", custom.display()),
    )
    .unwrap();
    std::fs::write(
        home.0.join("home/.zshrc"),
        format!(
            "echo 'login profile output'\nexport PATH='{}:/usr/bin:/bin'\n",
            home.0.join("bin").display()
        ),
    )
    .unwrap();
    let ran = Command::new(env!("CARGO_BIN_EXE_muster"))
        .args(["harness", "refresh", "codex", "--login"])
        .env_clear()
        .env("HOME", home.0.join("home"))
        .env("SHELL", "/bin/zsh")
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert!(text(&ran.stdout).contains("refreshed"), "{}", text(&ran.stdout));
    assert!(!text(&ran.stdout).contains("login profile output"));
    assert_eq!(
        std::fs::read(copy.join("hooks/hooks.json")).unwrap(),
        std::fs::read(Path::new(EXTRAS).join("codex/hooks/hooks.json")).unwrap()
    );
    assert_eq!(
        home.asked("codex"),
        [
            "plugin marketplace list --json".to_string(),
            format!("plugin marketplace add {}", home.extras().display()),
            "plugin add muster-codex@muster".to_string(),
        ]
    );
}

impl Home {
    fn agy_fixture(&self, listing: &str) {
        let linked = self.extras();
        std::fs::rename(&linked, linked.with_file_name("original-extras")).unwrap();
        let plugin = linked.join("agy");
        std::fs::create_dir_all(&plugin).unwrap();
        for (name, file) in [
            ("plugin.json", r#"{"name":"muster"}"#),
            ("hooks.json", r#"{"muster":{}}"#),
            ("report.py", "# fixture report"),
            ("statusline.sh", "# fixture status line"),
            ("README.md", "# fixture readme"),
        ] {
            std::fs::write(plugin.join(name), file).unwrap();
        }
        let config = self.0.join("home/.gemini/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("hooks.json"), br#"{"another-tool":{}}"#).unwrap();
        let settings = self.0.join("home/.gemini/antigravity-cli");
        std::fs::create_dir_all(&settings).unwrap();
        std::fs::write(settings.join("settings.json"), br#"{"statusLine":{"enabled":false}}"#)
            .unwrap();
        let path = self.0.join("bin/agy");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n\
             case \"$1 $2\" in\n\
             'plugin list') printf '%s\\n' '{}';;\n\
             'plugin install') /bin/mkdir -p '{}' && /bin/cp -R \"$3/.\" '{}';;\n\
             *) exit 91;;\nesac\n",
            self.ran("agy").display(),
            listing,
            self.agy_copy().display(),
            self.agy_copy().display(),
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn agy_copy(&self) -> PathBuf {
        self.0.join("home/.gemini/config/plugins/muster")
    }

    fn agy_listing(path: &Path, enabled: bool) -> String {
        serde_json::json!([{
            "name": "muster", "enabled": enabled, "version": "0.1.0",
            "source": "antigravity", "path": path, "hooks": ["SessionStart"]
        }])
        .to_string()
    }

    fn assert_agy_settings_unchanged(&self) {
        let config = self.0.join("home/.gemini/config");
        assert_eq!(std::fs::read(config.join("hooks.json")).unwrap(), br#"{"another-tool":{}}"#);
        assert_eq!(
            std::fs::read(self.0.join("home/.gemini/antigravity-cli/settings.json")).unwrap(),
            br#"{"statusLine":{"enabled":false}}"#
        );
    }
}

#[test]
fn antigravity_install_uses_only_the_native_plugin_command() {
    let home = Home::new("agy-install");
    home.agy_fixture("[]");
    let dry = home.muster(&["--json", "harness", "install", "agy", "--dry-run"]);
    assert_eq!(dry.status.code(), Some(0), "{}", text(&dry.stderr));
    assert!(home.asked("agy").is_empty());
    let printed: serde_json::Value = serde_json::from_slice(&dry.stdout).unwrap();
    let command = format!("agy plugin install {}", home.extras().join("agy").display());
    assert_eq!(printed["commands"], serde_json::json!([command]));
    assert_eq!(printed["ran"], false);

    let install = home.muster(&["harness", "install", "agy"]);
    assert_eq!(install.status.code(), Some(0), "{}", text(&install.stderr));
    assert_eq!(home.asked("agy"), [command.strip_prefix("agy ").unwrap()]);
    assert!(text(&install.stdout).contains("statusline.sh"), "{}", text(&install.stdout));
    assert!(home.agy_copy().join("hooks.json").is_file());
    home.assert_agy_settings_unchanged();
}

#[test]
fn antigravity_refresh_preserves_a_disabled_plugin_and_skips_a_matching_copy() {
    let home = Home::new("agy-refresh");
    home.agy_fixture(&Home::agy_listing(&home.agy_copy(), false));
    std::fs::create_dir_all(home.agy_copy()).unwrap();
    std::fs::write(home.agy_copy().join("hooks.json"), b"old").unwrap();
    let first = home.muster(&["--json", "harness", "refresh", "agy"]);
    assert_eq!(first.status.code(), Some(0), "{}", text(&first.stderr));
    let outcome: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(outcome["harness"], "agy");
    let outcome = outcome["outcome"].as_str().unwrap();
    assert!(outcome.contains("refreshed") && outcome.contains("disabled"), "{outcome}");
    assert_eq!(
        std::fs::read(home.agy_copy().join("hooks.json")).unwrap(),
        std::fs::read(home.extras().join("agy/hooks.json")).unwrap(),
    );

    std::fs::write(home.agy_copy().join("README.md"), b"old").unwrap();
    let second = home.muster(&["harness", "refresh", "agy"]);
    assert_eq!(second.status.code(), Some(0), "{}", text(&second.stderr));
    let said = text(&second.stdout);
    assert!(said.contains("matches") && said.contains("disabled"), "{said}");
    assert_eq!(
        home.asked("agy"),
        [
            "plugin list --json".to_string(),
            format!("plugin install {}", home.extras().join("agy").display()),
            "plugin list --json".to_string(),
            "plugin list --json".to_string(),
        ],
    );
    home.assert_agy_settings_unchanged();
}

#[test]
fn antigravity_refresh_has_no_first_install() {
    let home = Home::new("agy-absent");
    home.agy_fixture("[]");
    let refresh = home.muster(&["harness", "refresh", "agy"]);
    assert_eq!(refresh.status.code(), Some(0), "{}", text(&refresh.stderr));
    assert!(text(&refresh.stdout).contains("not installed"));
    assert_eq!(home.asked("agy"), ["plugin list --json"]);
    assert!(!home.agy_copy().exists());
}

#[test]
fn antigravity_refresh_refuses_malformed_or_outside_plugin_paths() {
    let cases = [("broken", "not JSON"), ("outside", ""), ("symlink", "")];
    for (name, invalid) in cases {
        let home = Home::new(name);
        let outside = home.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let path = if name == "symlink" {
            let copy = home.agy_copy();
            std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&outside, &copy).unwrap();
            copy
        } else {
            outside
        };
        let listing =
            if invalid.is_empty() { Home::agy_listing(&path, true) } else { invalid.to_string() };
        home.agy_fixture(&listing);
        let refresh = home.muster(&["harness", "refresh", "agy"]);
        assert_eq!(refresh.status.code(), Some(1), "{}", text(&refresh.stderr));
        let said = text(&refresh.stderr);
        assert!(said.contains("plugin listing") || said.contains("outside"), "{said}");
        assert_eq!(home.asked("agy"), ["plugin list --json"]);
        home.assert_agy_settings_unchanged();
    }
}
