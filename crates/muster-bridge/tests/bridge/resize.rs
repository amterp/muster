//! A resized window reaching the pane's program, with nothing faked.
//!
//! The bridge owns the PTY libghostty gave it, so the SIGWINCH a terminal sends stops at the
//! bridge: it only continues if the bridge sends the new grid to the daemon on the pane's
//! stream. Nothing downstream can notice that it did not - the pane keeps rendering at its old
//! geometry, and a full-screen program redraws into a grid the wrong shape, which reads as a
//! broken TUI rather than as a message that was never sent.
//!
//! It shipped broken for a reason no unit test would have found: SIGWINCH's default
//! disposition is to be ignored, and POSIX discards an ignored signal where it is generated
//! rather than leaving it pending, so the `sigwait` waiting for it never returned. Every
//! piece was correct in isolation. So this drives the real arrangement - a real PTY, a real
//! bridge, a real daemon - and asks the program in the pane what size its terminal is.

use std::io::Read;
use std::os::fd::FromRawFd;
use std::process::{Child, Command, Stdio};

use muster_daemon_proto::input_event;
use muster_harness::requests::{create, in_new_tab, make, read_text};
use muster_harness::{Daemon, Input, until};

const FIRST: (u16, u16) = (100, 30);
const AFTER: (u16, u16) = (120, 50);

/// The grid a bridge attaches with, and the grid it is resized to, are each the size the
/// pane's program reads from its own terminal.
#[test]
fn resizing_the_surface_resizes_the_pane() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));

    let terminal = Pty::open(FIRST);
    let mut bridge = terminal.run_bridge("p1", &daemon);

    // The program asks, over and over, rather than once after each resize: the resize travels
    // on the bridge's stream and a command typed afterwards on another connection, and nothing
    // orders the two.
    Input::connect(daemon.socket_path()).send(
        "p1",
        input_event::Input::Send(input_event::Send {
            text: "while :; do stty size; sleep 0.1; done".to_string(),
            enter: true,
            ..Default::default()
        }),
    );

    // The size the bridge attached with, which it reads from the PTY. Asserted first so that a
    // failure below is about the resize rather than about the bridge never having been the
    // right size at all.
    until(
        "the pane's program to see the size the bridge attached with",
        || screen(&daemon).lines().any(|line| line.trim_end() == stty(FIRST)),
        || format!("the pane shows:\n{}", screen(&daemon)),
    );

    terminal.resize(AFTER, &bridge);

    until(
        "the pane's program to see the size the window was resized to",
        || screen(&daemon).lines().any(|line| line.trim_end() == stty(AFTER)),
        || {
            format!(
                "the pane shows:\n{}\n  The resize never reached the program: either the \
                 bridge did not see the signal, or what it sent was not acted on. A \
                 `bridge.resize` record in the bridge's log separates those.",
                screen(&daemon)
            )
        },
    );

    // Killed and reaped: a leaked bridge holds a stream open against a daemon that is about
    // to be torn down under it.
    let _ = bridge.kill();
    let _ = bridge.wait();
}

/// What `stty size` prints for this size: rows, then columns.
fn stty((columns, rows): (u16, u16)) -> String {
    format!("{rows} {columns}")
}

/// Every row the daemon holds for the pane, scrollback included.
fn screen(daemon: &Daemon) -> String {
    read_text(&mut daemon.connect(), "p1", 0, 0).text
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
        terminal.discard_what_is_drawn();
        terminal
    }

    /// Reads the bridge's output and drops it, as a surface would read and draw it.
    ///
    /// Unread, the terminal fills up and the bridge blocks writing to it, which would make this
    /// a test of what a bridge does with a surface that stopped reading. The reader ends when
    /// the terminal is closed.
    fn discard_what_is_drawn(&self) {
        // SAFETY: a fresh duplicate of a descriptor this struct owns, handed to a File that
        // closes it.
        let mut primary = unsafe { std::fs::File::from_raw_fd(libc::dup(self.primary)) };
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = primary.read(&mut buffer)
                && read > 0
            {}
        });
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
            .arg("--daemon-socket")
            .arg(daemon.socket_path())
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
