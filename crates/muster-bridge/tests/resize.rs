//! A resized window reaching the pane's program, with nothing faked.
//!
//! The bridge owns the PTY libghostty gave it and `herdr pane attach` is its child on a pipe,
//! so the SIGWINCH a terminal sends stops at the bridge: it only continues if the bridge
//! translates it into a `terminal.resize` on the control stream. Nothing downstream can
//! notice that it did not - the pane keeps rendering at its old geometry, and a full-screen
//! program redraws into a grid the wrong shape, which reads as a broken TUI rather than as a
//! message that was never sent.
//!
//! It shipped broken for a reason no unit test would have found: SIGWINCH's default
//! disposition is to be ignored, and POSIX discards an ignored signal where it is generated
//! rather than leaving it pending, so the `sigwait` waiting for it never returned. Every
//! piece was correct in isolation. So this drives the real arrangement - a real PTY, a real
//! bridge, a real daemon - and asks the daemon what size it thinks the pane is.

use std::os::fd::FromRawFd;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use herdr_harness::Daemon;
use serde_json::json;

const FIRST: (u16, u16) = (100, 30);
const AFTER: (u16, u16) = (120, 50);

#[test]
fn resizing_the_surface_resizes_the_pane() {
    let daemon = Daemon::start();
    let created = daemon.call("workspace.create", &json!({ "focus": true, "label": null }));
    let pane = created["root_pane"]["pane_id"]
        .as_str()
        .unwrap_or_else(|| panic!("herdr answered a workspace.create without a pane: {created}"))
        .to_string();

    let terminal = Pty::open(FIRST);
    let mut bridge = terminal.run_bridge(&pane, &daemon);

    // The size the bridge reports when it starts, which it reads from the PTY and passes to
    // `herdr pane attach`. Asserted first so that a failure below is about the resize rather
    // than about the bridge never having been the right size at all.
    until(
        "the daemon to render the pane at the size the bridge attached with",
        || rows(&daemon, &pane) == Some(FIRST.1),
        || format!("the daemon says {:?} rows", rows(&daemon, &pane)),
    );

    terminal.resize(AFTER, &bridge);

    until(
        "the daemon to render the pane at the size the window was resized to",
        || rows(&daemon, &pane) == Some(AFTER.1),
        || {
            format!(
                "the daemon still says {:?} rows. The resize never reached it: either the \
                 bridge did not see the signal, or what it sent was not acted on. A \
                 `bridge.resize` record in the run log separates those.",
                rows(&daemon, &pane)
            )
        },
    );

    // Killed and reaped: a leaked bridge holds a control stream open against a daemon that
    // is about to be torn down under it.
    let _ = bridge.kill();
    let _ = bridge.wait();
}

/// How many rows the daemon is rendering a pane for, by its own account.
fn rows(daemon: &Daemon, pane: &str) -> Option<u16> {
    let listed = daemon.call("pane.list", &json!({}));
    listed["panes"]
        .as_array()?
        .iter()
        .find(|held| held["pane_id"].as_str() == Some(pane))?["scroll"]["viewport_rows"]
        .as_u64()
        .and_then(|rows| u16::try_from(rows).ok())
}

/// A real pseudo-terminal, standing in for the one libghostty hands a bridge.
struct Pty {
    primary: i32,
    replica: i32,
}

impl Pty {
    fn open(size: (u16, u16)) -> Pty {
        let mut primary = 0;
        let mut replica = 0;
        // SAFETY: openpty writes two descriptors we own and nothing else; the null arguments
        // are the documented way to ask for the defaults.
        let opened = unsafe {
            libc::openpty(
                &raw mut primary,
                &raw mut replica,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(opened, 0, "no pseudo-terminal could be opened, so there is nothing to resize");
        let terminal = Pty { primary, replica };
        terminal.resize_without_signalling(size);
        terminal
    }

    /// What a window resize is: a new size on the terminal, and a signal to whoever is
    /// reading it.
    ///
    /// The signal is sent by hand because the kernel sends it to the terminal's foreground
    /// process group, and a bridge spawned by a test is not in one. Under libghostty it
    /// arrives on its own.
    fn resize(&self, size: (u16, u16), reader: &Child) {
        self.resize_without_signalling(size);
        // SAFETY: kill takes a pid and a signal number and touches nothing of ours.
        unsafe { libc::kill(reader.id() as libc::pid_t, libc::SIGWINCH) };
    }

    fn resize_without_signalling(&self, (columns, rows): (u16, u16)) {
        let size = libc::winsize { ws_row: rows, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: TIOCSWINSZ reads a winsize we own and writes nothing back.
        unsafe { libc::ioctl(self.primary, libc::TIOCSWINSZ, &raw const size) };
    }

    /// Starts a bridge with this terminal as its stdin and stdout, the way a surface does.
    fn run_bridge(&self, pane: &str, daemon: &Daemon) -> Child {
        // The bridge runs `herdr` off PATH when it is not told otherwise, which would reach
        // for whichever version the developer happens to have. The pinned one goes first.
        let pinned = herdr_harness::binary();
        let directory = std::path::Path::new(&pinned)
            .parent()
            .unwrap_or_else(|| panic!("the pinned herdr at {pinned} has no directory"));
        let path = format!("{}:{}", directory.display(), std::env::var("PATH").unwrap_or_default());

        // SAFETY: both are fresh duplicates of a descriptor this struct owns, handed to
        // Stdio which closes them.
        let (stdin, stdout) = unsafe {
            (
                Stdio::from_raw_fd(libc::dup(self.replica)),
                Stdio::from_raw_fd(libc::dup(self.replica)),
            )
        };

        Command::new(env!("CARGO_BIN_EXE_muster-bridge"))
            .arg(pane)
            .args(["--herdr-socket", &daemon.socket_path().to_string_lossy()])
            .env("PATH", path)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::null())
            .spawn()
            .expect("cargo builds muster-bridge before this test runs")
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // SAFETY: both descriptors are ours and are closed once.
        unsafe {
            libc::close(self.primary);
            libc::close(self.replica);
        }
    }
}

/// Waits for something a daemon has to say, rather than sleeping and hoping.
fn until(what: &str, mut ready: impl FnMut() -> bool, on_failure: impl FnOnce() -> String) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}.\n{}", on_failure());
}
