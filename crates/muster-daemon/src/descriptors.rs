//! Keeping the daemon's descriptors out of the programs it starts.
//!
//! Marking each descriptor close-on-exec as it is opened is not enough. std's `accept` on macOS
//! accepts and then marks the new socket in a second call, and a pane forked in between would
//! keep that client's socket for as long as it runs; a descriptor the daemon inherited from its
//! launcher without the flag leaks the same way. So a new pane marks every descriptor above
//! stderr close-on-exec itself, between fork and exec, whatever the daemon holds. Marked rather
//! than closed, so std's own exec-error pipe still reports a failed exec.
//!
//! And how many a pane's programs may hold, which they inherit from the daemon.

/// The soft `RLIMIT_NOFILE` the daemon asks for on macOS, and so every pane's programs with it.
///
/// Launch Services starts the daemon with launchd's 256. A daemon the app spawned inherited
/// the app's own, which asks for this number (`DescriptorLimit.swift`), so asking here keeps a
/// pane's programs where they were. Not elsewhere: a daemon on Linux starts from an ssh
/// session's limit, which is what a shell on that machine has.
const WANTED: libc::rlim_t = 4096;

/// Raises the daemon's soft descriptor limit to [`WANTED`] where it is lower, and says what it
/// is now.
pub(crate) fn raise_limit() -> std::io::Result<libc::rlim_t> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes one rlimit into `limit`.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let wanted = WANTED.min(limit.rlim_max);
    if !cfg!(target_os = "macos") || limit.rlim_cur >= wanted {
        return Ok(limit.rlim_cur);
    }
    let raised = libc::rlimit { rlim_cur: wanted, rlim_max: limit.rlim_max };
    // SAFETY: setrlimit reads one rlimit, and a soft limit at or below the hard one is allowed.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const raised) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(wanted)
}

/// What the child needs to mark its descriptors, gathered in the parent: nothing may be
/// allocated between fork and exec.
#[derive(Debug)]
pub(crate) struct Sealing {
    /// The highest descriptor number a process can hold, for the fallback that tries them all.
    limit: i32,
    #[cfg(target_os = "macos")]
    listed: Vec<libc::proc_fdinfo>,
}

impl Sealing {
    pub(crate) fn prepare() -> Sealing {
        // SAFETY: sysconf only reads a system limit.
        let limit = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
        let limit = i32::try_from(limit).unwrap_or(i32::MAX).clamp(256, 1 << 20);
        Sealing {
            limit,
            #[cfg(target_os = "macos")]
            listed: macos::buffer(),
        }
    }

    /// Marks every descriptor above stderr close-on-exec. Runs in the child between fork and
    /// exec, so it only makes async-signal-safe calls and allocates nothing.
    pub(crate) fn seal(&mut self) {
        if self.seal_listed() {
            return;
        }
        for descriptor in 3..self.limit {
            // SAFETY: fcntl on a descriptor number; one that is not open answers EBADF.
            unsafe {
                libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
    }

    /// The quick way, where the kernel offers one. False when it could not be relied on.
    #[cfg(target_os = "linux")]
    #[allow(clippy::unused_self)]
    fn seal_listed(&mut self) -> bool {
        // SAFETY: close_range with CLOSE_RANGE_CLOEXEC only sets a flag on a range of
        // descriptors. Kernels before 5.11 refuse the flag, and the caller falls back.
        let marked = unsafe {
            libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, libc::CLOSE_RANGE_CLOEXEC)
        };
        marked == 0
    }

    #[cfg(target_os = "macos")]
    fn seal_listed(&mut self) -> bool {
        macos::seal(&mut self.listed)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    const ENTRY: usize = size_of::<libc::proc_fdinfo>();

    /// Room for every descriptor open now and some opened before the fork.
    pub(super) fn buffer() -> Vec<libc::proc_fdinfo> {
        // SAFETY: a null buffer asks proc_pidinfo only how many bytes a listing needs.
        let needed = unsafe {
            libc::proc_pidinfo(libc::getpid(), libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0)
        };
        let entries = usize::try_from(needed).unwrap_or(0) / ENTRY + 64;
        vec![libc::proc_fdinfo { proc_fd: -1, proc_fdtype: 0 }; entries]
    }

    /// Lists the child's descriptors into `listed` and marks each. False if the listing failed
    /// or may have been cut short by the buffer's size.
    pub(super) fn seal(listed: &mut [libc::proc_fdinfo]) -> bool {
        let Ok(size) = i32::try_from(size_of_val(listed)) else { return false };
        // SAFETY: `listed` is valid for `size` bytes of writes. proc_pidinfo is a system call
        // wrapper that takes no locks and allocates nothing.
        let written = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                listed.as_mut_ptr().cast(),
                size,
            )
        };
        if written <= 0 || written >= size {
            return false;
        }
        let count = usize::try_from(written).unwrap_or(0) / ENTRY;
        for entry in &listed[..count] {
            if entry.proc_fd > 2 {
                // SAFETY: fcntl on a descriptor the listing just named.
                unsafe {
                    libc::fcntl(entry.proc_fd, libc::F_SETFD, libc::FD_CLOEXEC);
                }
            }
        }
        true
    }
}
