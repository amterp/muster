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
//! group unless one leaves it, which a Bash tool's shell does. A harness's helper - its own
//! binary run under a name its manifest lists, as Codex runs its Linux sandbox - is not an agent,
//! so a walk goes past it to the agent that started it.
//!
//! A harness's server is where a walk stops instead, and the report is refused whatever pane it
//! names. Codex's shared app-server runs the commands and hooks of every Codex session on the
//! machine, all with the environment of whichever session started it, and stays its child while
//! that session runs: walking past it would find that session's agent and file every other
//! session's reports under its pane (`docs/observations/codex-0.160.0.md`, section 1).

use std::collections::HashSet;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::{Mutex, OnceLock};

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_detect::{Manifests, Processes, System, identify_process, server_of};

/// How far up a walk goes before it gives up and accepts. A hook sits two or three processes
/// below its agent.
const DEPTH: usize = 32;

/// How many offending processes are remembered as already logged, before the memory starts over.
const LOGGED: usize = 256;

/// Who sent a report, as far as a walk up from its sender can say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Sender {
    Agent(Nearest),
    Server(Server),
}

/// The nearest process above a report's sender that detection would call an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Nearest {
    pub(crate) pid: u32,
    pub(crate) group: u32,
    pub(crate) agent: String,
}

/// A harness's binary above a report's sender, serving sessions other than its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Server {
    pub(crate) pid: u32,
    pub(crate) agent: String,
    pub(crate) subcommand: String,
    pub(crate) remedy: String,
}

impl Server {
    /// Why a report through this server is refused, and what a person does about it.
    pub(crate) fn refusal(&self) -> String {
        format!(
            "this report came through `{} {}` (pid {}), which runs the commands and hooks of every \
             {} session on this machine with the environment of the session that started it, so \
             it cannot say which pane it is from. To fix it, {}",
            self.agent, self.subcommand, self.pid, self.agent, self.remedy
        )
    }
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

/// The nearest agent or server at or above `sender` on this machine, if a walk finds one.
pub(crate) fn sender(sender: u32, manifests: &Manifests) -> Option<Sender> {
    sender_with(sender, own_ancestors(), &System, manifests)
}

/// The walk itself, over any process source. Stops without an answer at pid 1, at a process that
/// is this daemon or above it - whatever started the daemon is not in any pane, though a test
/// suite run from inside an agent has one there - at a process it cannot read, and after
/// [`DEPTH`] steps.
pub(crate) fn sender_with(
    sender: u32,
    above_the_daemon: &HashSet<u32>,
    processes: &impl Processes,
    manifests: &Manifests,
) -> Option<Sender> {
    let mut pid = sender;
    for _ in 0..DEPTH {
        if pid <= 1 || above_the_daemon.contains(&pid) {
            return None;
        }
        let placed = processes.placed(pid)?;
        if let Some((agent, server)) = server_of(&placed.process, manifests) {
            return Some(Sender::Server(Server {
                pid,
                agent: agent.id().to_string(),
                subcommand: server.subcommand,
                remedy: server.remedy,
            }));
        }
        if let Some(agent) = identify_process(&placed.process, manifests) {
            let agent = agent.id().to_string();
            return Some(Sender::Agent(Nearest { pid, group: placed.group, agent }));
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

/// Whether a refusal of a report from `pid` is yet to be logged: an agent's hooks report on every
/// turn, and one line per process says all there is to say.
fn first_refusal(pid: u32) -> bool {
    static LOGGED_PIDS: Mutex<Option<HashSet<u32>>> = Mutex::new(None);
    let mut logged = poison::lock(&LOGGED_PIDS, "daemon.report.refused");
    let logged = logged.get_or_insert_with(HashSet::new);
    if logged.len() >= LOGGED {
        logged.clear();
    }
    logged.insert(pid)
}

/// Logs a report refused because `nearest` sent it, once per process.
pub(crate) fn refused(pane: &str, own: &str, nearest: &Nearest) {
    if !first_refusal(nearest.pid) {
        return;
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

/// Logs a report refused because it came through `server`, once per server.
pub(crate) fn refused_through_server(pane: &str, server: &Server) {
    if !first_refusal(server.pid) {
        return;
    }
    log::warn(
        "daemon.report.shared_server",
        fields! {
            "pane" => pane,
            "sender_agent" => server.agent,
            "server" => format!("{} {}", server.agent, server.subcommand),
            "server_pid" => server.pid,
            "why" => "the report came from below a server that runs the work of every session of \
                      its harness on this machine, with the environment of whichever session \
                      started it, so the pane it names may not be the one it came from",
            "impact" => "every report through this server is refused, so its sessions' panes are \
                         read off their screens, and an agent's own `report --waiting` fails; \
                         this is logged once per server",
            "check" => server.remedy,
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

    /// The walk's answer, which these cases expect to be an agent when there is one.
    fn nearest(
        sender: u32,
        above_the_daemon: &HashSet<u32>,
        processes: &impl Processes,
        manifests: &Manifests,
    ) -> Option<Nearest> {
        match sender_with(sender, above_the_daemon, processes, manifests)? {
            Sender::Agent(nearest) => Some(nearest),
            Sender::Server(server) => panic!("the walk stopped at a server: {server:?}"),
        }
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
        let own = nearest(40, &daemon, &table(), &manifests()).unwrap();
        assert_eq!((own.pid, own.group, own.agent.as_str()), (20, 20, "claude"));
        let nested = nearest(80, &daemon, &table(), &manifests()).unwrap();
        assert_eq!((nested.pid, nested.group), (60, 50), "past the hook's shell to the nested one");
    }

    /// Codex on Linux, as measured on a devenv: the pane's shell 100 runs `codex` 110 in group
    /// 110, which runs each command under its sandbox helper 120, its own binary under another
    /// argv[0], in a group of its own. The model's own report 140 is the pane's codex's. A
    /// `codex exec` 150 started through that helper is a nested agent, and so is one 180 that a
    /// shell given only that command exec'd into, a child of the pane's codex in a group of its
    /// own: which agent sits directly above is no test, and the helper's name is.
    #[test]
    fn a_codex_helper_is_walked_past_and_a_nested_codex_exec_is_not() {
        let table = Table(HashMap::from([
            (100, ("zsh", vec!["-zsh"], 2, 100)),
            (110, ("codex", vec!["codex"], 100, 110)),
            (120, ("codex", vec!["codex-linux-sandbox", "--sandbox-policy", "{}"], 110, 120)),
            (130, ("bash", vec!["/bin/bash", "-lc", "\"$MUSTER_DAEMON\" report"], 120, 120)),
            (140, ("muster-daemon", vec!["muster-daemon", "report"], 130, 120)),
            (150, ("codex", vec!["codex", "exec", "hi"], 130, 120)),
            (160, ("sh", vec!["/bin/sh", "-c", "\"$MUSTER_DAEMON\" report"], 150, 120)),
            (170, ("muster-daemon", vec!["muster-daemon", "report"], 160, 120)),
            (180, ("codex", vec!["codex", "exec", "hi"], 110, 180)),
            (190, ("sh", vec!["/bin/sh", "-c", "\"$MUSTER_DAEMON\" report"], 180, 180)),
            (195, ("muster-daemon", vec!["muster-daemon", "report"], 190, 180)),
        ]));
        let daemon = HashSet::from([2]);
        let own = nearest(140, &daemon, &table, &manifests()).unwrap();
        assert_eq!((own.pid, own.group), (110, 110), "the helper is the pane's codex's");
        let nested = nearest(170, &daemon, &table, &manifests()).unwrap();
        assert_eq!((nested.pid, nested.group), (150, 120));
        let exec_d = nearest(195, &daemon, &table, &manifests()).unwrap();
        assert_eq!((exec_d.pid, exec_d.group), (180, 180));
    }

    /// Codex 0.160.0's shared app-server, as measured on macOS: pane A's shell 100 runs `codex`
    /// 110, which starts the server 120 as its child in a group of its own. The server runs the
    /// hooks and commands of A's session and of pane B's `codex` 210 alike, all with A's
    /// environment. Walking past it would find 110 for B's report 160 too, so the walk stops at it,
    /// and still does once A has exited and the server is launchd's.
    #[test]
    fn a_walk_stops_at_a_shared_server_whoever_started_it() {
        let server = || {
            vec![
                "/Users/me/.codex/packages/app-server-daemon/releases/0.160.0/bin/codex",
                "app-server",
                "--listen",
                "unix://",
                "--managed-daemon",
            ]
        };
        let rows = |server_parent: u32| {
            Table(HashMap::from([
                (100, ("zsh", vec!["-zsh"], 2, 100)),
                (110, ("codex", vec!["codex"], 100, 110)),
                (120, ("codex", server(), server_parent, 120)),
                (130, ("sh", vec!["/bin/sh", "-c", "\"$MUSTER_DAEMON\" report"], 120, 130)),
                (140, ("muster-daemon", vec!["muster-daemon", "report"], 130, 130)),
                (150, ("zsh", vec!["/bin/zsh", "-c", "report --waiting"], 120, 150)),
                (160, ("muster-daemon", vec!["muster-daemon", "report"], 150, 150)),
                (200, ("zsh", vec!["-zsh"], 2, 200)),
                (210, ("codex", vec!["codex"], 200, 210)),
            ]))
        };
        let daemon = HashSet::from([2]);
        for (table, when) in [(rows(110), "while A runs"), (rows(1), "once A has exited")] {
            for report in [140, 160] {
                let Some(Sender::Server(server)) =
                    sender_with(report, &daemon, &table, &manifests())
                else {
                    panic!("report {report} {when} was not stopped at the server");
                };
                assert_eq!((server.pid, server.agent.as_str()), (120, "codex"), "{when}");
                let refusal = server.refusal();
                assert!(refusal.contains("`codex app-server` (pid 120)"), "{refusal}");
                assert!(refusal.contains("codex features disable daemon_auto_start"), "{refusal}");
            }
        }
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
            let _ = sender_with(std::process::id(), &HashSet::new(), &System, &manifests);
        }
        let each = started.elapsed() / walks;
        println!("one walk up this machine's process tree took {each:?}");
        assert!(each < std::time::Duration::from_millis(50), "a walk took {each:?}");
    }

    #[test]
    fn a_walk_that_finds_no_agent_answers_nothing() {
        let daemon = HashSet::from([2]);
        assert_eq!(nearest(90, &daemon, &table(), &manifests()), None, "an orphan");
        assert_eq!(nearest(10, &daemon, &table(), &manifests()), None, "a shell");
        assert_eq!(nearest(99, &daemon, &table(), &manifests()), None, "unreadable");
        let above = HashSet::from([2, 20]);
        assert_eq!(
            nearest(40, &above, &table(), &manifests()),
            None,
            "an agent above the daemon is nobody's in a pane"
        );
    }
}
