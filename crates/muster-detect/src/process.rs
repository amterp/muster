//! What the kernel says about the processes in a pane's foreground. One arm per OS Muster
//! runs a daemon on.
//!
//! Ported from herdr v0.8.0 `src/platform/{mod,macos,linux}.rs` (Apache-2.0), and changed:
//! the foreground group is handed in by the daemon, which reads it off the PTY it holds, rather
//! than looked up from the shell's pid; Linux's child-groups fallback for terminals that do not
//! report a foreground group is gone, since the daemon's always do; and the agent hint is
//! `MUSTER_AGENT`, returned as the name it gives rather than resolved here.

#[cfg(target_os = "macos")]
mod macos;

// The /proc parsing and tree walk are plain functions of their inputs, so they are compiled
// for the tests on every OS; only the reads that feed them need Linux.
#[cfg(any(target_os = "linux", test))]
mod linux;

/// One process in a foreground job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    /// The kernel's name for it: `comm`, truncated as the kernel truncates it.
    pub name: String,
    /// The basename of its `argv[0]`, where the OS reports it separately (macOS). A runtime
    /// that renames itself - Node's `process.title` - shows here and not in `name`.
    pub argv0: Option<String>,
    /// Its arguments, when the kernel will say. Another user's process usually will not.
    pub argv: Option<Vec<String>>,
}

/// The processes in a terminal's foreground process group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub group: u32,
    pub processes: Vec<Process>,
}

impl Job {
    pub(crate) fn leader(&self) -> Option<&Process> {
        self.processes.iter().find(|process| process.pid == self.group)
    }
}

/// Where process facts come from: the running system, or a test's own.
pub trait Processes {
    /// The group's leader alone - cheaper than the whole job, and usually enough.
    fn leader(&self, group: u32) -> Option<Job>;

    /// Every process in the group. `shell` is the pane's own process, which Linux walks the
    /// process tree from.
    fn job(&self, shell: u32, group: u32) -> Option<Job>;

    /// The `MUSTER_AGENT` a process was started with, if it was: the name of the agent it
    /// says it is, which overrides whatever its executable is called. macOS withholds the
    /// environment of its own platform binaries (`/bin/sleep`, `/bin/zsh`), so a hint is only
    /// ever read from a process that is not one - which no agent is.
    fn agent_hint(&self, pid: u32) -> Option<String>;
}

/// The processes of the machine this runs on.
#[derive(Debug, Clone, Copy, Default)]
pub struct System;

impl Processes for System {
    fn leader(&self, group: u32) -> Option<Job> {
        #[cfg(target_os = "macos")]
        return macos::leader(group);
        #[cfg(target_os = "linux")]
        return linux::leader(group);
    }

    fn job(&self, shell: u32, group: u32) -> Option<Job> {
        #[cfg(target_os = "macos")]
        return macos::job(shell, group);
        #[cfg(target_os = "linux")]
        return linux::job(shell, group);
    }

    fn agent_hint(&self, pid: u32) -> Option<String> {
        #[cfg(target_os = "macos")]
        return macos::agent_hint(pid);
        #[cfg(target_os = "linux")]
        return linux::agent_hint(pid);
    }
}

const AGENT_HINT: &[u8] = b"MUSTER_AGENT=";

/// The first `MUSTER_AGENT` in a NUL-separated environment block. The first decides, as the
/// C library's `getenv` would: a later duplicate is not the process's value.
fn agent_hint_in(environ: &[u8]) -> Option<String> {
    let value =
        environ.split(|&byte| byte == 0).find_map(|record| record.strip_prefix(AGENT_HINT))?;
    std::str::from_utf8(value).ok().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_agent_hint_decides() {
        let environ = b"HOME=/x\0MUSTER_AGENT=claude\0MUSTER_AGENT=codex\0";
        assert_eq!(agent_hint_in(environ).as_deref(), Some("claude"));
        assert_eq!(agent_hint_in(b"HERDR_AGENT=claude\0"), None);
        assert_eq!(agent_hint_in(b"MUSTER_AGENT=\xff\0"), None);
    }

    const CHILD: &str = "MUSTER_DETECT_TEST_CHILD";

    /// Stands in for an agent in the test below. Run as a child of this test binary with
    /// `CHILD` set, it sleeps; in an ordinary run it does nothing. A child of its own rather
    /// than `/bin/sleep`, because macOS does not show a platform binary's environment, and
    /// the hint is read from there.
    #[test]
    fn stand_in_agent() {
        if let Some(seconds) = std::env::var(CHILD).ok().and_then(|value| value.parse().ok()) {
            std::thread::sleep(std::time::Duration::from_secs(seconds));
        }
    }

    fn until<T>(mut ready: impl FnMut() -> Option<T>) -> Option<T> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let value = ready();
            if value.is_some() || std::time::Instant::now() > deadline {
                return value;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// The system's own answer about a real process group: a child started in a group of its
    /// own, as a pane's foreground job is.
    #[test]
    fn the_system_reports_a_real_process_group() {
        use std::os::unix::process::CommandExt;

        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exec sleep 30"])
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        // `exec` replaces the shell; the probe has to see sleep, not sh.
        let job = until(|| {
            System.job(pid, pid).filter(|job| job.processes.iter().any(|p| p.name == "sleep"))
        });
        let leader = System.leader(pid);
        let _ = child.kill();
        let _ = child.wait();

        let job = job.expect("the child's group should be visible, running sleep");
        assert_eq!(job.group, pid);
        let sleep = job.leader().expect("the child leads its group");
        assert_eq!(sleep.argv.as_deref(), Some(&["sleep".to_string(), "30".to_string()][..]));
        assert_eq!(leader.map(|job| job.processes), Some(job.processes.clone()));
        assert_eq!(System.job(pid, pid), None, "a group that has gone has no job");
    }

    #[test]
    fn the_system_reads_an_agent_hint_from_a_process_environment() {
        use std::os::unix::process::CommandExt;

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "process::tests::stand_in_agent", "--test-threads=1"])
            .env(CHILD, "30")
            .env("MUSTER_AGENT", "claude")
            .process_group(0)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let hint = until(|| System.agent_hint(pid));
        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(hint.as_deref(), Some("claude"));
        assert_eq!(System.agent_hint(std::process::id()), None, "this process has no hint");
    }
}
