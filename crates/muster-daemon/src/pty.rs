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
use crate::spawn;

/// A pane's size in cells, and the pixels those cells cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Grid {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) width_px: u16,
    pub(crate) height_px: u16,
}

impl Grid {
    /// What a pane gets when nothing says how big it is: a terminal's traditional size.
    pub(crate) const FALLBACK: Grid = Grid { cols: 80, rows: 24, width_px: 0, height_px: 0 };

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

/// The process group in the foreground of a pane's terminal: the shell at its prompt, or
/// whatever the shell is running.
pub(crate) fn foreground_group(master: BorrowedFd<'_>) -> Option<i32> {
    // SAFETY: tcgetpgrp only reads the terminal's state.
    let group = unsafe { libc::tcgetpgrp(master.as_raw_fd()) };
    (group > 0).then_some(group)
}

/// Sends SIGHUP to a process group, which is what a terminal closing means to what runs in it.
pub(crate) fn hang_up(group: i32) {
    // SAFETY: killpg with a positive group id signals that group and nothing else. A group that
    // has already gone answers ESRCH, which is the outcome wanted anyway.
    unsafe {
        libc::killpg(group, libc::SIGHUP);
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

/// The argv for a pane given the configured shell settings.
pub(crate) fn argv(
    configured: Option<&str>,
    login: bool,
    runs_command: bool,
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
) -> Vec<String> {
    let shell = configured.map_or_else(|| default_shell(environment), str::to_string);
    spawn::argv(&shell, login, runs_command)
}
