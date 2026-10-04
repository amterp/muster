//! Which agent sent a report (MIP-3, section 8).
//!
//! A report names its pane through `$MUSTER_PANE`, which every process started in the pane
//! inherits: a `claude -p` the agent runs from its Bash tool, or an agent in a tmux server
//! started there. Their hooks would report into the outer agent's pane. The connection says who
//! sent the report, so the daemon walks up from the sender to the nearest process detection
//! would call an agent, and the session compares that process's group with the group the pane's
//! own agent was found in.
//!
//! Only a positive answer refuses. A sender this daemon cannot see, a lookup that fails, a walk
//! that reaches no agent - a report backgrounded until its hook exited, which leaves it to
//! launchd - or a pane with no identified agent all accept, as every report was accepted before.
//! Process groups rather than pids, because a harness started through a wrapper is several
//! processes that all read as the agent, and its hooks hang off any of them; they share the
//! group unless one leaves it, which a Bash tool's shell does.

use std::collections::HashSet;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::{Mutex, OnceLock};

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_detect::{Manifests, Processes, System, identify_process};

/// How far up a walk goes before it gives up and accepts. A hook sits two or three processes
/// below its agent.
const DEPTH: usize = 32;

/// How many offending processes are remembered as already logged, before the memory starts over.
const LOGGED: usize = 256;

/// The nearest process above a report's sender that detection would call an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Nearest {
    pub(crate) pid: u32,
    pub(crate) group: u32,
    pub(crate) agent: String,
}

/// The process on the other end of a connection, when the OS will say.
pub(crate) fn peer_pid(stream: &UnixStream) -> Option<u32> {
    let fd = stream.as_raw_fd();
    #[cfg(target_os = "macos")]
    {
        let mut pid: libc::pid_t = 0;
        let mut size = libc::socklen_t::try_from(size_of::<libc::pid_t>()).ok()?;
        // SAFETY: `pid` is valid for `size` bytes of writes, which is what LOCAL_PEERPID writes.
        let read = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                (&raw mut pid).cast(),
                &raw mut size,
            )
        };
        (read == 0).then_some(pid).and_then(|pid| u32::try_from(pid).ok()).filter(|&p| p > 0)
    }
    #[cfg(target_os = "linux")]
    {
        let mut credentials = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut size = libc::socklen_t::try_from(size_of::<libc::ucred>()).ok()?;
        // SAFETY: `credentials` is valid for `size` bytes of writes, which SO_PEERCRED writes.
        let read = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&raw mut credentials).cast(),
                &raw mut size,
            )
        };
        (read == 0)
            .then_some(credentials.pid)
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|&p| p > 0)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = fd;
        None
    }
}

/// The nearest agent at or above `sender` on this machine, if a walk finds one.
pub(crate) fn nearest_agent(sender: u32, manifests: &Manifests) -> Option<Nearest> {
    nearest_agent_with(sender, own_ancestors(), &System, manifests)
}

/// The walk itself, over any process source. Stops without an answer at pid 1, at a process that
/// is this daemon or above it - whatever started the daemon is not in any pane, though a test
/// suite run from inside an agent has one there - at a process it cannot read, and after
/// [`DEPTH`] steps.
pub(crate) fn nearest_agent_with(
    sender: u32,
    above_the_daemon: &HashSet<u32>,
    processes: &impl Processes,
    manifests: &Manifests,
) -> Option<Nearest> {
    let mut pid = sender;
    for _ in 0..DEPTH {
        if pid <= 1 || above_the_daemon.contains(&pid) {
            return None;
        }
        let placed = processes.placed(pid)?;
        if let Some(agent) = identify_process(&placed.process, manifests) {
            return Some(Nearest { pid, group: placed.group, agent: agent.id().to_string() });
        }
        pid = placed.parent;
    }
    None
}

/// This daemon and every process above it, read once.
fn own_ancestors() -> &'static HashSet<u32> {
    static OWN: OnceLock<HashSet<u32>> = OnceLock::new();
    OWN.get_or_init(|| {
        let mut own = HashSet::new();
        let mut pid = std::process::id();
        while pid > 1 && own.len() < DEPTH && own.insert(pid) {
            let Some(placed) = System.placed(pid) else { break };
            pid = placed.parent;
        }
        own
    })
}

/// Logs a report refused because `nearest` sent it, once per process: a nested agent's hooks
/// report on every turn, and one line says all there is to say.
pub(crate) fn refused(pane: &str, own: &str, nearest: &Nearest) {
    static LOGGED_PIDS: Mutex<Option<HashSet<u32>>> = Mutex::new(None);
    {
        let mut logged = poison::lock(&LOGGED_PIDS, "daemon.report.refused");
        let logged = logged.get_or_insert_with(HashSet::new);
        if logged.len() >= LOGGED {
            logged.clear();
        }
        if !logged.insert(nearest.pid) {
            return;
        }
    }
    log::warn(
        "daemon.report.foreign_agent",
        fields! {
            "pane" => pane,
            "agent" => own,
            "sender_agent" => nearest.agent,
            "sender_pid" => nearest.pid,
            "why" => "the report came from an agent process outside the process group the \
                      pane's own agent runs in, which a pane's environment passed on to it",
            "impact" => "its reports are refused while it runs, so the pane shows its own agent's \
                         state and facts rather than the nested one's; this is logged once per \
                         process",
            "check" => "what the pane's agent started: a `claude -p`, `codex exec` or \
                        `opencode run` from its shell, or a tmux server started in the pane",
        },
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use muster_detect::{Job, Placed, Process};

    use super::*;

    /// A process table: pid to (name, argv, parent, group).
    struct Table(HashMap<u32, (&'static str, Vec<&'static str>, u32, u32)>);

    impl Processes for Table {
        fn leader(&self, _: u32) -> Option<Job> {
            None
        }
        fn job(&self, _: u32, _: u32) -> Option<Job> {
            None
        }
        fn agent_hint(&self, _: u32) -> Option<String> {
            None
        }
        fn placed(&self, pid: u32) -> Option<Placed> {
            let (name, argv, parent, group) = self.0.get(&pid)?.clone();
            Some(Placed {
                process: Process {
                    pid,
                    name: name.to_string(),
                    argv0: None,
                    argv: Some(argv.into_iter().map(str::to_string).collect()),
                },
                parent,
                group,
            })
        }
    }

    fn manifests() -> Manifests {
        Manifests::built_in()
    }

    /// The pane's shell 10 runs `claude` 20 in group 20, whose hook shell 30 runs the report 40;
    /// its Bash tool's shell 50 leaves the group, and runs a `claude -p` 60 whose hook shell 70
    /// runs a report 80. The daemon is 2.
    fn table() -> Table {
        Table(HashMap::from([
            (10, ("zsh", vec!["-zsh"], 2, 10)),
            (20, ("claude", vec!["claude"], 10, 20)),
            (30, ("sh", vec!["/bin/sh", "-c", "\"$MUSTER_DAEMON\" report"], 20, 20)),
            (40, ("muster-daemon", vec!["muster-daemon", "report"], 30, 20)),
            (50, ("zsh", vec!["/bin/zsh", "-c", "claude -p hi"], 20, 50)),
            (60, ("claude", vec!["claude", "-p", "hi"], 50, 50)),
            (70, ("sh", vec!["/bin/sh", "-c", "\"$MUSTER_DAEMON\" report"], 60, 50)),
            (80, ("muster-daemon", vec!["muster-daemon", "report"], 70, 50)),
            (90, ("muster-daemon", vec!["muster-daemon", "report"], 1, 90)),
        ]))
    }

    #[test]
    fn a_hook_is_its_own_agents_and_a_nested_agents_hook_is_the_nested_agents() {
        let daemon = HashSet::from([2]);
        let own = nearest_agent_with(40, &daemon, &table(), &manifests()).unwrap();
        assert_eq!((own.pid, own.group, own.agent.as_str()), (20, 20, "claude"));
        let nested = nearest_agent_with(80, &daemon, &table(), &manifests()).unwrap();
        assert_eq!((nested.pid, nested.group), (60, 50), "past the hook's shell to the nested one");
    }

    /// What a walk costs on this machine, from this test up through whatever ran it - when run
    /// from inside an agent, up to that agent - printed for a person to read. A statusline
    /// reports every few seconds, and the walk runs before the session's lock is taken.
    #[test]
    fn a_walk_up_this_machines_process_tree_costs_little() {
        let manifests = manifests();
        let walks = 100;
        let started = std::time::Instant::now();
        for _ in 0..walks {
            let _ = nearest_agent_with(std::process::id(), &HashSet::new(), &System, &manifests);
        }
        let each = started.elapsed() / walks;
        println!("one walk up this machine's process tree took {each:?}");
        assert!(each < std::time::Duration::from_millis(50), "a walk took {each:?}");
    }

    #[test]
    fn a_walk_that_finds_no_agent_answers_nothing() {
        let daemon = HashSet::from([2]);
        assert_eq!(nearest_agent_with(90, &daemon, &table(), &manifests()), None, "an orphan");
        assert_eq!(nearest_agent_with(10, &daemon, &table(), &manifests()), None, "a shell");
        assert_eq!(nearest_agent_with(99, &daemon, &table(), &manifests()), None, "unreadable");
        let above = HashSet::from([2, 20]);
        assert_eq!(
            nearest_agent_with(40, &above, &table(), &manifests()),
            None,
            "an agent above the daemon is nobody's in a pane"
        );
    }
}
