//! The stand-in `ssh`, and what it was asked.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use muster_ssh::{Forward, Report, Tunnel};

/// Answers the three shapes of ssh Muster runs: a master (`-M`), a control request (`-O`), and
/// a command over a master. Everything it is asked goes to `<control path>.d/log`, one line
/// each, so tests running side by side keep separate books.
///
/// A master makes both its paths exist and sleeps as itself, so the pid Muster holds is the
/// master's. `-O check` answers from whether that master still runs, unless `slow` says to sit
/// past the supervisor's patience first. `-O exit` ends the master and removes the control path,
/// which is what ssh does.
const SCRIPT: &str = r#"#!/bin/sh
ctl=""; near=""; mode="run"; op=""
while [ $# -gt 0 ]; do
  case "$1" in
    -S) ctl="$2"; shift 2 ;;
    -L) near="${2%%:*}"; shift 2 ;;
    -M) mode="master"; shift ;;
    -O) mode="control"; op="$2"; shift 2 ;;
    -o|-p|-F) shift 2 ;;
    -N) shift ;;
    *) break ;;
  esac
done
state="$ctl.d"
mkdir -p "$state"
echo "$mode $op" >> "$state/log"
case "$mode" in
  master)
    : > "$near"; : > "$ctl"; echo $$ > "$state/master"
    exec sleep 600 ;;
  control)
    case "$op" in
      check)
        slow=$(cat "$state/slow" 2>/dev/null || echo 0)
        if [ "$slow" -gt 0 ]; then echo $((slow - 1)) > "$state/slow"; exec sleep 30; fi
        if [ -e "$ctl" ] && kill -0 "$(cat "$state/master")" 2>/dev/null; then exit 0; fi
        echo "Control socket connect($ctl): No such file or directory" >&2
        exit 255 ;;
      exit)
        kill "$(cat "$state/master")" 2>/dev/null; rm -f "$ctl"; exit 0 ;;
      *) exit 0 ;;
    esac ;;
  *) exit 0 ;;
esac
"#;

/// Puts the stand-in first on this process's `PATH`, once.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let directory = home();
        std::fs::create_dir_all(&directory).expect("the fake ssh's directory should be makeable");
        // Staged and renamed, because two test runs share this directory and one must never
        // exec a script the other is halfway through writing.
        let staged = directory.join(format!("ssh.{}", std::process::id()));
        std::fs::write(&staged, SCRIPT).expect("the fake ssh should be writable");
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .expect("the fake ssh should be made executable");
        std::fs::rename(&staged, directory.join("ssh")).expect("the fake ssh should be placed");
        let path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: inside `call_once`, which every test in this binary passes through before it
        // runs anything, so no other thread of this process reads the environment while it is
        // written, and nothing writes it afterwards.
        unsafe { std::env::set_var("PATH", format!("{}:{path}", directory.display())) };
    });
}

/// Where the stand-in lives, and every test's paths under it. One directory for every run, so
/// that runs do not leave one each behind.
fn home() -> PathBuf {
    std::env::temp_dir().join("muster-fake-ssh")
}

/// A directory of its own for one test's tunnel, short enough for a unix socket path, and gone
/// when the test is.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new() -> Scratch {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        install();
        let directory =
            home().join(format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&directory).expect("the scratch directory should be makeable");
        Scratch(directory)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A forward whose paths are all inside `directory`.
pub(crate) fn forward(directory: &Path) -> Forward {
    Forward {
        host: "devenv".to_string(),
        options: Vec::new(),
        control_path: directory.join("m.ctl").to_string_lossy().into_owned(),
        local_socket: directory.join("m.sock").to_string_lossy().into_owned(),
        remote_socket: "/home/dev/.muster/daemon/d.sock".to_string(),
        reverse: None,
    }
}

pub(crate) fn open(forward: Forward) -> Tunnel {
    let report: Report = std::sync::Arc::new(|_| {});
    Tunnel::open(forward, report).expect("the fake master should open")
}

/// Makes the next `count` checks on this control path sit past the supervisor's patience.
pub(crate) fn slow_checks(control_path: &str, count: u32) {
    let state = format!("{control_path}.d");
    std::fs::create_dir_all(&state).expect("the fake's state directory should be makeable");
    std::fs::write(format!("{state}/slow"), count.to_string()).expect("slow should be writable");
}

/// Every line the stand-in logged for this control path, in order.
pub(crate) fn asked(control_path: &str) -> Vec<String> {
    std::fs::read_to_string(format!("{control_path}.d/log"))
        .unwrap_or_default()
        .lines()
        .map(|line| line.trim().to_string())
        .collect()
}

pub(crate) fn count(control_path: &str, line: &str) -> usize {
    asked(control_path).iter().filter(|asked| *asked == line).count()
}

/// Polls until `done`, or panics saying what was asked by then.
pub(crate) fn until(what: &str, control_path: &str, within: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + within;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "gave up waiting for {what}; the stand-in ssh was asked {:?}",
            asked(control_path)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
