//! A daemon that stops once a named process exits, for a test that must not leave it running.
//!
//! A test's own teardown cannot end its daemon when the test is killed, and a group kill reaches
//! the daemon but not its panes, each of which leads a session of its own. So a test names its
//! own pid in [`VARIABLE`], and the daemon stops when that process exits, the way it stops on
//! SIGTERM: every pane hung up, and whatever ignores the hang-up killed after the grace. A
//! successor started by a handoff inherits the environment, so it watches the same process.
//!
//! Nothing but a test sets it. A person's daemon outlives whatever started it (MIP-3).

use std::sync::mpsc::Sender;
use std::time::Duration;

use muster_core::diagnostics::log;
use muster_core::fields;

use crate::session::Stop;

/// The pid whose exit ends this daemon.
pub(crate) const VARIABLE: &str = "MUSTER_DAEMON_ENDS_WITH";

/// What [`VARIABLE`] asks for.
#[derive(Debug, PartialEq, Eq)]
enum Asked {
    Nothing,
    EndWith(i32),
    Unreadable(String),
}

fn asked(value: Option<&str>) -> Asked {
    match value.map(str::trim) {
        None | Some("") => Asked::Nothing,
        Some(text) => match text.parse::<i32>() {
            Ok(pid) if pid > 1 => Asked::EndWith(pid),
            _ => Asked::Unreadable(text.to_string()),
        },
    }
}

/// Starts watching the process [`VARIABLE`] names, if it names one, and asks the daemon to stop
/// once that process has exited.
pub(crate) fn watch(stopping: Sender<Stop>) -> Result<(), String> {
    let pid = match asked(std::env::var(VARIABLE).ok().as_deref()) {
        Asked::Nothing => return Ok(()),
        Asked::EndWith(pid) => pid,
        Asked::Unreadable(value) => {
            log::warn(
                "daemon.ends_with.invalid",
                fields! {
                    "value" => value,
                    "impact" => "this daemon keeps running after whatever set the variable \
                                 exits, holding its panes",
                    "check" => "the variable takes the pid of a process that is running; \
                                only a test harness sets it",
                },
            );
            return Ok(());
        }
    };
    log::info("daemon.ends_with.watching", fields! { "owner_pid" => pid });
    std::thread::Builder::new()
        .name("ends-with".to_string())
        .spawn(move || {
            wait_for_exit(pid);
            log::info("daemon.ends_with.exited", fields! { "owner_pid" => pid });
            let _ = stopping.send(Stop::Asked);
        })
        .map(|_| ())
        .map_err(|error| format!("could not start the thread watching pid {pid}: {error}"))
}

/// Returns once `pid` has exited, or at once if it is not running.
#[cfg(target_os = "macos")]
fn wait_for_exit(pid: i32) {
    // SAFETY: kqueue takes no arguments; the descriptor is closed below.
    let queue = unsafe { libc::kqueue() };
    if queue == -1 {
        return poll_for_exit(pid, &std::io::Error::last_os_error());
    }
    // SAFETY: a zeroed kevent is a valid value; the fields set are the ones EVFILT_PROC reads.
    let mut change: libc::kevent = unsafe { std::mem::zeroed() };
    change.ident = pid.cast_unsigned() as libc::uintptr_t;
    change.filter = libc::EVFILT_PROC;
    change.flags = libc::EV_ADD | libc::EV_ONESHOT;
    change.fflags = libc::NOTE_EXIT;
    // SAFETY: as above.
    let mut event: libc::kevent = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: one change in and room for one event out, both valid for the call; a null
        // timeout waits for as long as it takes.
        let got = unsafe {
            libc::kevent(queue, &raw const change, 1, &raw mut event, 1, std::ptr::null())
        };
        if got == -1 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        break;
    }
    // A registration refused with ESRCH arrives as an event carrying EV_ERROR: the process had
    // already gone, which is the answer too.
    // SAFETY: the descriptor is this function's.
    unsafe { libc::close(queue) };
}

#[cfg(target_os = "linux")]
fn wait_for_exit(pid: i32) {
    // SAFETY: pidfd_open takes a pid and flags and returns a descriptor this function owns.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if descriptor == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return;
        }
        return poll_for_exit(pid, &error);
    }
    #[allow(clippy::cast_possible_truncation)]
    let descriptor = descriptor as i32;
    let mut readable = libc::pollfd { fd: descriptor, events: libc::POLLIN, revents: 0 };
    // SAFETY: one pollfd, valid for the call; -1 waits for as long as it takes. A pidfd reads
    // as ready once its process has exited.
    while unsafe { libc::poll(&raw mut readable, 1, -1) } == -1
        && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
    {}
    // SAFETY: the descriptor is this function's.
    unsafe { libc::close(descriptor) };
}

/// The fallback when the kernel will not watch the process for us: asks every half second
/// whether the pid is still in use, which a reused pid can fool.
fn poll_for_exit(pid: i32, why: &std::io::Error) {
    log::warn(
        "daemon.ends_with.polling",
        fields! {
            "owner_pid" => pid,
            "error" => why,
            "impact" => "the daemon notices the exit up to half a second late, and keeps \
                         running if the pid is reused by then",
            "check" => "whether this kernel supports kqueue process events or pidfd_open",
        },
    );
    // SAFETY: kill with signal 0 only asks whether the process exists.
    while unsafe { libc::kill(pid, 0) } == 0 {
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::{Asked, asked};

    #[test]
    fn a_pid_is_watched_and_anything_else_is_not() {
        assert_eq!(asked(None), Asked::Nothing);
        assert_eq!(asked(Some("")), Asked::Nothing);
        assert_eq!(asked(Some(" 4242 ")), Asked::EndWith(4242));
        for unreadable in ["abc", "-5", "0", "1", "4242x"] {
            assert_eq!(asked(Some(unreadable)), Asked::Unreadable(unreadable.to_string()));
        }
    }
}
