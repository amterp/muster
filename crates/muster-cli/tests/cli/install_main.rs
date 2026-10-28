//! Exercise install-main's real detached swap with process and Launch Services edges replaced.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const INSTALL_MAIN: &str = include_str!("../../../../tools/install-main");
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
const CALLER_VARIABLES: &[(&str, &str)] = &[
    ("MUSTER_PANE", "another-pane"),
    ("MUSTER_CALLSIGN", "another-agent"),
    ("MUSTER_SOCKET", "a-gone-window.sock"),
    ("MUSTER_DAEMON_SOCKET", "another-daemon.sock"),
    ("CLAUDECODE", "1"),
    ("CLAUDE_CODE_SESSION_ID", "another-session"),
    ("CLAUDE_CODE_MESSAGING_SOCKET", "another-inbox.sock"),
    ("CLAUDE_CODE_MESSAGING_TOKEN", "inbox-token-sentinel"),
    ("QM_JOB", "a-finished-job"),
    ("QM_JOBSERVER", "another-jobserver"),
    ("GHOSTTY_BIN_DIR", "/another/terminal"),
    ("TERM_PROGRAM", "ghostty"),
    ("UNRELATED_SENTINEL", "must-not-reach-the-app"),
];

const EDGES: &str = r#"
pgrep() {
  if [ "$FIXTURE_MODE" != stopped ]; then printf '%s\n' "$$"; fi
}
ps() { printf '%s/Installed Muster.app/Contents/MacOS/muster\n' "$FIXTURE_ROOT"; }
kill() { [ "$1" = -TERM ]; }
mv() {
  if [ "$FIXTURE_MODE" = move-failure ] && [ "$1" = "$FIXTURE_ROOT/Installed Muster.app" ]; then
    return 1
  fi
  /bin/mv "$@"
}
ditto() {
  if [ "$FIXTURE_MODE" = copy-failure ]; then
    /bin/mkdir -p "$2"
    printf partial > "$2/version"
    return 1
  fi
  /usr/bin/ditto "$@"
}
rm() {
  [ "$#" = 2 ] && [ "$1" = -rf ] && [ "$2" = "$FIXTURE_ROOT/Installed Muster.app" ] || return 1
  /bin/mv "$2" "$FIXTURE_ROOT/partial-copy"
}
open() { "$FIXTURE_ROOT/open" "$@"; }
env() {
  local forwarded=() arg
  for arg in "$@"; do
    case "$arg" in
      open|/usr/bin/open) forwarded+=("$FIXTURE_ROOT/open") ;;
      *) forwarded+=("$arg") ;;
    esac
  done
  /usr/bin/env "${forwarded[@]}"
}
"#;

const OPEN: &str = r#"#!/bin/bash
printf '%s\n' "$@" > "${0%/*}/open.args"
/bin/cat "$1/version" > "${0%/*}/open.version"
/usr/bin/env > "${0%/*}/open.env"
"#;

struct Swap {
    root: PathBuf,
}

impl Swap {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("muster install {} {name}", std::process::id()));
        std::fs::create_dir(&root).expect("each swap has a fresh scratch directory");
        let fixture = Self { root };
        for (name, version) in [("Installed Muster.app", "old"), ("Staged Muster.app", "new")] {
            std::fs::create_dir(fixture.root.join(name)).unwrap();
            std::fs::write(fixture.root.join(name).join("version"), version).unwrap();
        }
        let (_, rest) = INSTALL_MAIN.split_once("SWAP = r\"\"\"\n").expect("the swap body exists");
        let (body, _) = rest.split_once("\n\"\"\"").expect("the swap body ends");
        std::fs::write(fixture.root.join("swap.sh"), body).unwrap();
        std::fs::write(fixture.root.join("edges.sh"), EDGES).unwrap();
        std::fs::write(
            fixture.root.join("run.sh"),
            "source \"$1\"\nsource \"$2\" \"$3\" \"$4\" \"$5\" \"$6\"\n",
        )
        .unwrap();
        let open = fixture.root.join("open");
        std::fs::write(&open, OPEN).unwrap();
        std::fs::set_permissions(open, std::fs::Permissions::from_mode(0o755)).unwrap();
        fixture
    }

    fn app(&self) -> PathBuf {
        self.root.join("Installed Muster.app")
    }

    fn run(&self, mode: &str) -> Output {
        Command::new("/bin/bash")
            .arg(self.root.join("run.sh"))
            .arg(self.root.join("edges.sh"))
            .arg(self.root.join("swap.sh"))
            .arg(self.app())
            .arg(self.root.join("Staged Muster.app"))
            .arg(self.root.join("Previous Muster.app"))
            .arg(self.root.join("workspace home"))
            .env_clear()
            .env("HOME", self.root.join("login home"))
            .env("USER", "owner")
            .env("LOGNAME", "owner-login")
            .env("SHELL", "/bin/zsh")
            .env("TMPDIR", self.root.join("temporary files"))
            .env("LANG", "en_US.UTF-8")
            .env("PATH", "/caller-only:/usr/bin:/bin")
            .env("MUSTER_HOME", "the-callers-home")
            .env("FIXTURE_ROOT", &self.root)
            .env("FIXTURE_MODE", mode)
            .envs(CALLER_VARIABLES.iter().copied())
            .stdin(Stdio::null())
            .output()
            .expect("the isolated swap runs")
    }

    fn opened(&self, version: &str) {
        assert_eq!(self.read("open.args"), format!("{}\n", self.app().display()));
        assert_eq!(self.read("open.version"), version);
        let recorded = self.read("open.env");
        let environment: BTreeMap<_, _> =
            recorded.lines().filter_map(|line| line.split_once('=')).collect();
        for (key, _) in CALLER_VARIABLES {
            assert!(!environment.contains_key(key), "{key} reached the app: {recorded}");
        }
        for (key, expected) in [
            ("HOME", self.root.join("login home").display().to_string()),
            ("USER", "owner".to_string()),
            ("LOGNAME", "owner-login".to_string()),
            ("SHELL", "/bin/zsh".to_string()),
            ("TMPDIR", self.root.join("temporary files").display().to_string()),
            ("LANG", "en_US.UTF-8".to_string()),
            ("PATH", SYSTEM_PATH.to_string()),
            ("MUSTER_HOME", self.root.join("workspace home").display().to_string()),
        ] {
            assert_eq!(environment.get(key).copied(), Some(expected.as_str()), "{key}: {recorded}");
        }
        assert!(!environment.contains_key("FIXTURE_ROOT"), "unlisted variables reached the app");
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).unwrap()
    }
}

impl Drop for Swap {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_replacement_app_gets_the_login_environment_and_its_selected_home() {
    let swap = Swap::new("success");
    let ran = swap.run("success");
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    assert_eq!(swap.read("Installed Muster.app/version"), "new");
    assert_eq!(swap.read("Previous Muster.app/version"), "old");
    swap.opened("new");
}

#[test]
fn an_app_reopened_after_a_move_failure_drops_the_callers_environment() {
    let swap = Swap::new("move-failure");
    let ran = swap.run("move-failure");
    assert_eq!(ran.status.code(), Some(1));
    assert_eq!(swap.read("Installed Muster.app/version"), "old");
    assert!(!swap.root.join("Previous Muster.app").exists());
    swap.opened("old");
}

#[test]
fn an_app_restored_after_a_copy_failure_drops_the_callers_environment() {
    let swap = Swap::new("copy-failure");
    let ran = swap.run("copy-failure");
    assert_eq!(ran.status.code(), Some(1));
    assert_eq!(swap.read("Installed Muster.app/version"), "old");
    assert!(!swap.root.join("Previous Muster.app").exists());
    assert_eq!(swap.read("partial-copy/version"), "partial");
    swap.opened("old");
}

#[test]
fn replacing_an_app_that_was_not_running_opens_nothing() {
    let swap = Swap::new("stopped");
    let ran = swap.run("stopped");
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    assert_eq!(swap.read("Installed Muster.app/version"), "new");
    assert!(!swap.root.join("open.env").exists());
}
