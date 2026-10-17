//! Opening a pane's PTY and starting its process on it.
//!
//! A pane gets its terminal on stdin, stdout and stderr and no other descriptor of the daemon's,
//! whatever another thread opened a moment before the fork (`descriptors.rs`).

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::descriptors::Sealing;

/// A pane's size in cells, and the pixels those cells cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Grid {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) width_px: u16,
    pub(crate) height_px: u16,
}

impl Grid {
    /// What a pane gets when nothing says how big it is: a terminal's traditional size.
    pub(crate) const FALLBACK: Grid = Grid { cols: 80, rows: 24, width_px: 0, height_px: 0 };

    /// The grid as one word, so a pane can publish its size without a lock.
    pub(crate) fn to_bits(self) -> u64 {
        u64::from(self.cols)
            | u64::from(self.rows) << 16
            | u64::from(self.width_px) << 32
            | u64::from(self.height_px) << 48
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn from_bits(bits: u64) -> Grid {
        Grid {
            cols: bits as u16,
            rows: (bits >> 16) as u16,
            width_px: (bits >> 32) as u16,
            height_px: (bits >> 48) as u16,
        }
    }

    fn winsize(self) -> libc::winsize {
        libc::winsize {
            ws_row: self.rows,
            ws_col: self.cols,
            ws_xpixel: self.width_px,
            ws_ypixel: self.height_px,
        }
    }
}

/// What starts a pane: the program and its arguments, its environment, and where.
#[derive(Debug)]
pub(crate) struct Launch<'a> {
    pub(crate) argv: &'a [String],
    pub(crate) environment: &'a [(std::ffi::OsString, std::ffi::OsString)],
    pub(crate) cwd: &'a Path,
    pub(crate) grid: Grid,
}

/// Opens a PTY at `launch.grid` and starts the program on it, in a session of its own with the
/// PTY as its controlling terminal. Returns the master and the child.
pub(crate) fn start(launch: &Launch<'_>) -> io::Result<(OwnedFd, Child)> {
    let (master, slave) = open(launch.grid)?;
    let (program, arguments) =
        launch.argv.split_first().ok_or_else(|| io::Error::other("an empty argv"))?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .env_clear()
        .envs(launch.environment.iter().map(|(name, value)| (name, value)))
        .current_dir(launch.cwd)
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    let mut sealing = Sealing::prepare();
    // SAFETY: the closure runs in the child between fork and exec, where only async-signal-safe
    // calls are allowed. sigemptyset, sigprocmask, setsid, ioctl and what `seal` calls all are,
    // and it touches no memory the parent's other threads might have held locked at the fork.
    unsafe {
        command.pre_exec(move || {
            // std passes the forking thread's signal mask on, and the daemon blocks SIGHUP,
            // SIGINT and SIGTERM for the thread that waits on them (`main.rs`). A pane started
            // with those blocked never hears its terminal hang up.
            let mut unblocked = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
            libc::sigemptyset(unblocked.as_mut_ptr());
            if libc::sigprocmask(libc::SIG_SETMASK, unblocked.as_ptr(), std::ptr::null_mut()) == -1
            {
                return Err(io::Error::last_os_error());
            }
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            // Stdin is the slave by now; claiming it makes the PTY this session's controlling
            // terminal, which is what delivers SIGHUP when it closes and job control within it.
            // `as _` because the constant's type and ioctl's request type differ between
            // macOS and the Linux libcs, and the cast is widening on some and a no-op on others.
            #[allow(clippy::cast_lossless)]
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            sealing.seal();
            Ok(())
        });
    }
    let child = command.spawn()?;
    Ok((master, child))
}

fn open(grid: Grid) -> io::Result<(OwnedFd, OwnedFd)> {
    let mut master = -1;
    let mut slave = -1;
    let mut size = grid.winsize();
    // SAFETY: both out-pointers are valid for writes; a null name and termios ask for the
    // defaults; `size` outlives the call.
    let opened = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    };
    if opened == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty succeeded, so both are open descriptors this function now owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    close_on_exec(&master)?;
    close_on_exec(&slave)?;
    Ok((master, slave))
}

fn close_on_exec(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor this process owns.
    let set = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    if set == -1 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Tells a pane's terminal, and so its program, a new size.
pub(crate) fn set_size(master: BorrowedFd<'_>, grid: Grid) -> io::Result<()> {
    let size = grid.winsize();
    // SAFETY: TIOCSWINSZ reads one winsize, which outlives the call. `as _` as for TIOCSCTTY.
    #[allow(clippy::cast_lossless)]
    let set = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &raw const size) };
    if set == -1 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// The process group in the foreground of a pane's terminal: the shell at its prompt, or
/// whatever the shell is running.
pub(crate) fn foreground_group(master: BorrowedFd<'_>) -> Option<i32> {
    // SAFETY: tcgetpgrp only reads the terminal's state.
    let group = unsafe { libc::tcgetpgrp(master.as_raw_fd()) };
    (group > 0).then_some(group)
}

/// What a pane's terminal holds for its program to read: how many bytes, and whether the
/// terminal is line by line, in which case only whole lines count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Queued {
    pub(crate) bytes: u32,
    pub(crate) canonical: bool,
}

/// Reads what the pane's program has not yet read, by opening the terminal's slave side for a
/// moment. Only the slave says it: the master's queue is the program's output.
///
/// Opened and closed each time, never kept: the reader learns a pane has ended from the master
/// reading EOF, which comes only once every descriptor of the slave is closed.
pub(crate) fn queued(master: BorrowedFd<'_>) -> io::Result<Queued> {
    let slave = open_slave(master)?;
    let mut bytes: libc::c_int = 0;
    // SAFETY: FIONREAD writes one int, which outlives the call. `as _` as for TIOCSCTTY.
    #[allow(clippy::cast_lossless)]
    if unsafe { libc::ioctl(slave.as_raw_fd(), libc::FIONREAD as _, &raw mut bytes) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let mut modes = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr fills the termios it is given. The slave's, not the master's: on Linux
    // the master has modes of its own.
    if unsafe { libc::tcgetattr(slave.as_raw_fd(), modes.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: tcgetattr succeeded and filled it.
    let modes = unsafe { modes.assume_init() };
    Ok(Queued {
        bytes: u32::try_from(bytes).unwrap_or(0),
        canonical: modes.c_lflag & libc::ICANON != 0,
    })
}

/// Throws away what the pane's program has not yet read. Only the slave can: flushing the
/// master's input would throw away the program's output instead.
pub(crate) fn flush_input(master: BorrowedFd<'_>) -> io::Result<()> {
    let slave = open_slave(master)?;
    // SAFETY: tcflush only changes the terminal's queue.
    if unsafe { libc::tcflush(slave.as_raw_fd(), libc::TCIFLUSH) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The terminal's slave side, opened for a moment ([`queued`] says why never for longer).
fn open_slave(master: BorrowedFd<'_>) -> io::Result<OwnedFd> {
    let name = slave_name(master)?;
    // SAFETY: `name` is a NUL-terminated path. O_NOCTTY keeps the terminal from becoming this
    // process's controlling terminal; O_NONBLOCK keeps a slow open from holding the reader.
    let fd = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: open succeeded, so the caller owns the descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(target_os = "linux")]
fn slave_name(master: BorrowedFd<'_>) -> io::Result<std::ffi::CString> {
    let mut buffer = [0 as libc::c_char; 128];
    // SAFETY: the buffer is as long as said, and ptsname_r NUL-terminates what it writes.
    let named = unsafe { libc::ptsname_r(master.as_raw_fd(), buffer.as_mut_ptr(), buffer.len()) };
    if named != 0 {
        return Err(io::Error::from_raw_os_error(named));
    }
    // SAFETY: ptsname_r succeeded, so the buffer holds a NUL-terminated name.
    Ok(unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }.to_owned())
}

#[cfg(target_vendor = "apple")]
fn slave_name(master: BorrowedFd<'_>) -> io::Result<std::ffi::CString> {
    // TIOCPTYGNAME's buffer is 128 bytes, as its number encodes.
    let mut buffer = [0 as libc::c_char; 128];
    // SAFETY: TIOCPTYGNAME writes at most 128 bytes, NUL-terminated, into the buffer.
    #[allow(clippy::cast_lossless)]
    if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCPTYGNAME as _, buffer.as_mut_ptr()) }
        == -1
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the ioctl succeeded, so the buffer holds a NUL-terminated name.
    Ok(unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }.to_owned())
}

/// The session the terminal belongs to, which is its leader's pid, or None once that leader has
/// gone and the terminal belongs to no session.
pub(crate) fn session_of(master: BorrowedFd<'_>) -> Option<i32> {
    // SAFETY: tcgetsid only reads the terminal's state.
    let session = unsafe { libc::tcgetsid(master.as_raw_fd()) };
    (session > 0).then_some(session)
}

/// Whether a process with this pid exists.
pub(crate) fn process_exists(pid: i32) -> bool {
    // SAFETY: kill with signal 0 only asks whether the process exists.
    let answered = unsafe { libc::kill(pid, 0) };
    answered == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Sends SIGHUP to a process group, which is what a terminal closing means to what runs in it.
pub(crate) fn hang_up(group: i32) {
    // SAFETY: killpg with a positive group id signals that group and nothing else. A group that
    // has already gone answers ESRCH, which is the outcome wanted anyway.
    unsafe {
        libc::killpg(group, libc::SIGHUP);
    }
}

/// Whether any process is left in a process group.
pub(crate) fn group_exists(group: i32) -> bool {
    // SAFETY: killpg with signal 0 only asks whether the group has a process to signal.
    unsafe { libc::killpg(group, 0) == 0 }
}

/// Asks a process group to end, as `kill` does by default.
pub(crate) fn terminate_group(group: i32) {
    // SAFETY: killpg with a positive group id signals that group and nothing else.
    unsafe {
        libc::killpg(group, libc::SIGTERM);
    }
}

/// Kills what is left of a process group.
pub(crate) fn kill_group(group: i32) {
    // SAFETY: killpg with a positive group id signals that group and nothing else.
    unsafe {
        libc::killpg(group, libc::SIGKILL);
    }
}

/// Ends a process started for a pane that will never exist, and reaps it: SIGKILL to its
/// process group, since nothing it started has had a chance to matter yet.
pub(crate) fn abandon(pid: i32) {
    // SAFETY: killpg and kill with a positive id signal that group or process only; the child
    // leads its own group because it started its own session. waitpid reaps this daemon's own
    // child and writes one int.
    unsafe {
        libc::killpg(pid, libc::SIGKILL);
        libc::kill(pid, libc::SIGKILL);
        let mut status = 0;
        while libc::waitpid(pid, &raw mut status, 0) == -1
            && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
        {}
    }
}

/// The shell a pane runs when nothing names one: `$SHELL`, else the account's, else `/bin/sh`.
pub(crate) fn default_shell(environment: &[(std::ffi::OsString, std::ffi::OsString)]) -> String {
    let from_environment = environment
        .iter()
        .find(|(name, _)| name == "SHELL")
        .and_then(|(_, value)| value.to_str())
        .filter(|shell| !shell.is_empty())
        .map(str::to_string);
    from_environment.or_else(account_shell).unwrap_or_else(|| "/bin/sh".to_string())
}

fn account_shell() -> Option<String> {
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0 as libc::c_char; 4096];
    let mut found = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call, and `buffer` is as long as it says.
    let status = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &raw mut found,
        )
    };
    if status != 0 || found.is_null() {
        return None;
    }
    // SAFETY: getpwuid_r succeeded and filled `entry`, whose strings point into `buffer`.
    let shell = unsafe { std::ffi::CStr::from_ptr((*found).pw_shell) };
    shell.to_str().ok().filter(|shell| !shell.is_empty()).map(str::to_string)
}

/// The shell a pane runs: the configured one, else the default.
pub(crate) fn shell(
    configured: Option<&str>,
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
) -> String {
    configured.map_or_else(|| default_shell(environment), str::to_string)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::fd::AsFd;

    use super::*;

    #[test]
    fn a_flush_leaves_nothing_for_the_program_to_read() {
        let (master, _slave) = open(Grid::FALLBACK).expect("a terminal");
        std::fs::File::from(master.try_clone().expect("a copy"))
            .write_all(b"typed\n")
            .expect("written");
        let waiting = || queued(master.as_fd()).expect("readable").bytes;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while waiting() == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(waiting(), 6);
        flush_input(master.as_fd()).expect("flushed");
        assert_eq!(waiting(), 0);
    }
}
