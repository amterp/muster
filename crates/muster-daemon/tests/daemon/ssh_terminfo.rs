//! `ssh` from a pane, through Ghostty's shell integration and Muster's stand-in for the
//! `ghostty +ssh` it calls (`packaging/muster-daemon-data/bin/ghostty`, `src/ssh.rs`), against
//! an `ssh` of the test's making that records what it was asked. `./dev --ssh` runs the same
//! against a real host.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::support::DAEMON_DATA;

/// Records each run's arguments, answers `-G` as a host `dev@devbox` would, and takes an install
/// as the real one would, failing it when `FAKE_SSH_INSTALL_FAILS` is set.
const FAKE_SSH: &str = r#"#!/bin/sh
printf '%s\n' "$(printf '%s ' "$@" | tr '\n' ' ')" >> "$FAKE_SSH_DIR/runs"
if [ "$1" = "-G" ]; then
    printf 'user dev\nhostname devbox\nport 2222\n'
    exit 0
fi
case "$*" in
    *ControlMaster=yes*)
        cat > "$FAKE_SSH_DIR/installed"
        [ -n "$FAKE_SSH_INSTALL_FAILS" ] && exit 1
        exit 0 ;;
esac
exit 0
"#;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("muster-ssh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/ssh"), FAKE_SSH).unwrap();
        std::fs::set_permissions(dir.join("bin/ssh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        Scratch(dir)
    }

    /// Runs what a pane's `ssh` wrapper runs, and returns every line the fake ssh recorded.
    fn ssh(&self, flags: &[&str], failing: bool) -> Vec<String> {
        self.ssh_with(flags, &["-p", "2222", "devbox"], failing)
    }

    /// [`Scratch::ssh`] with ssh's own arguments given.
    fn ssh_with(&self, flags: &[&str], arguments: &[&str], failing: bool) -> Vec<String> {
        self.ssh_through(env!("CARGO_BIN_EXE_muster-daemon"), flags, arguments, failing)
    }

    /// [`Scratch::ssh_with`] with the pane's `$MUSTER_DAEMON` given.
    fn ssh_through(
        &self,
        daemon: &str,
        flags: &[&str],
        arguments: &[&str],
        failing: bool,
    ) -> Vec<String> {
        let _ = std::fs::remove_file(self.0.join("runs"));
        let path = format!("{}:{}", self.0.join("bin").display(), std::env::var("PATH").unwrap());
        let mut command = Command::new(Path::new(DAEMON_DATA).join("bin/ghostty"));
        command
            .arg("+ssh")
            .args(flags)
            .arg("--")
            .args(arguments)
            .env("PATH", path)
            .env("MUSTER_HOME", self.0.join("home"))
            .env("MUSTER_DAEMON", daemon)
            .env("FAKE_SSH_DIR", &self.0);
        if failing {
            command.env("FAKE_SSH_INSTALL_FAILS", "1");
        }
        assert!(command.status().unwrap().success());
        std::fs::read_to_string(self.0.join("runs")).unwrap().lines().map(str::to_string).collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const FORWARDED: &str =
    "-o SendEnv=COLORTERM -o SendEnv=TERM_PROGRAM -o SendEnv=TERM_PROGRAM_VERSION -p 2222 devbox ";

/// The first ssh to a host installs the entry from the daemon's data over a connection of its
/// own, then connects as xterm-ghostty; the host is remembered, and the next ssh just connects.
#[test]
fn the_first_ssh_to_a_host_installs_the_entry_and_the_next_does_not() {
    let scratch = Scratch::new("install");
    let runs = scratch.ssh(&[], false);
    assert_eq!(runs.len(), 3, "{runs:?}");
    assert!(runs[0].starts_with("-G -p 2222 devbox"), "{runs:?}");
    assert!(runs[1].starts_with("-o ControlMaster=yes -o ControlPersist=no -o ControlPath="));
    assert!(runs[1].contains("-p 2222 devbox infocmp xterm-ghostty"), "{runs:?}");
    assert_eq!(runs[2], format!("-o SetEnv=TERM=xterm-ghostty {FORWARDED}"));
    let source = std::fs::read(Path::new(DAEMON_DATA).join("terminfo/ghostty.terminfo")).unwrap();
    assert_eq!(std::fs::read(scratch.0.join("installed")).unwrap(), source);

    let runs = scratch.ssh(&[], false);
    assert_eq!(runs.len(), 2, "installed once is enough: {runs:?}");
    assert_eq!(runs[1], format!("-o SetEnv=TERM=xterm-ghostty {FORWARDED}"));
}

/// A host that will not take the entry is reached as xterm-256color, and not remembered.
#[test]
fn a_host_that_will_not_take_the_entry_is_reached_as_xterm_256color() {
    let scratch = Scratch::new("refused");
    let runs = scratch.ssh(&[], true);
    assert_eq!(runs.last().unwrap(), &format!("-o SetEnv=TERM=xterm-256color {FORWARDED}"));
    let runs = scratch.ssh(&[], true);
    assert!(runs.iter().any(|run| run.contains("ControlMaster=yes")), "tried again: {runs:?}");
}

/// The wrapper passes `--forward-env=false` without `ssh-env` and `--terminfo=false` without
/// `ssh-terminfo`, as Ghostty's `+ssh` takes them.
#[test]
fn either_feature_off_leaves_its_part_out() {
    let scratch = Scratch::new("features");
    let runs = scratch.ssh(&["--forward-env=false"], false);
    assert_eq!(runs.last().unwrap(), "-p 2222 devbox ");
    let runs = scratch.ssh(&["--terminfo=false"], false);
    assert_eq!(runs, [format!("-o SetEnv=TERM=xterm-256color {FORWARDED}")]);
}

/// ssh that opens no terminal, or asks ssh rather than a host, is run exactly as given: an
/// install would hold a `-N` tunnel open at the install step, and a `-f` or `-G` that exits at
/// once would be taken for a host that has the entry.
#[test]
fn ssh_that_opens_no_terminal_is_run_as_given() {
    let scratch = Scratch::new("as-given");
    let cases: [&[&str]; 10] = [
        &["-N", "-L", "8080:localhost:80", "-p", "2222", "devbox"],
        &["-f", "-N", "-L", "8080:localhost:80", "devbox"],
        &["-fNL", "8080:localhost:80", "devbox"],
        &["-W", "localhost:22", "devbox"],
        &["-O", "check", "devbox"],
        &["-G", "devbox"],
        &["-V"],
        &["-Q", "cipher"],
        &["-s", "devbox", "sftp"],
        &["-p", "2222", "-N", "devbox"],
    ];
    for arguments in cases {
        let runs = scratch.ssh_with(&[], arguments, false);
        assert_eq!(runs, [format!("{} ", arguments.join(" "))], "{arguments:?}");
        assert!(!scratch.0.join("home/state/ssh-terminfo").exists(), "{arguments:?} was cached");
    }
}

/// An option's argument is not the destination, and a remote command is not sent with the
/// install: the install connection gets the options and the destination alone.
#[test]
fn the_install_gets_the_destination_without_the_remote_command() {
    let scratch = Scratch::new("command");
    let runs = scratch.ssh_with(
        &[],
        &["-o", "ControlMaster=no", "-p", "2222", "devbox", "ls", "-la"],
        false,
    );
    assert_eq!(runs.len(), 3, "{runs:?}");
    assert!(
        runs[1].contains("-o ControlMaster=no -p 2222 devbox infocmp xterm-ghostty"),
        "{runs:?}"
    );
    assert!(!runs[1].contains("ls -la"), "the command went with the install: {runs:?}");
    assert!(runs[2].ends_with("-o ControlMaster=no -p 2222 devbox ls -la "), "{runs:?}");
}

/// A pane outlives the daemon executable it was told about, which a reinstall or a moved bundle
/// takes away; its ssh then goes on as plain ssh rather than failing.
#[test]
fn a_daemon_that_is_gone_leaves_ssh_working() {
    let scratch = Scratch::new("gone");
    let runs =
        scratch.ssh_through("/nonexistent/muster-daemon", &[], &["-p", "2222", "devbox"], false);
    assert_eq!(runs, ["-p 2222 devbox "]);
}
