//! What the daemon's tests share: the daemon, and requests spelled so a test reads as what it
//! asks for.

#![allow(unreachable_pub)]

use std::path::Path;

pub use muster_daemon_proto as proto;
pub use muster_harness::requests::*;
pub use muster_harness::{Asked, Control, DAEMON_DATA, Daemon, until, until_file, until_some};

/// A daemon built from this commit, on a scratch root of its own.
pub fn daemon() -> Daemon {
    Daemon::start(env!("CARGO_BIN_EXE_muster-daemon"))
}

pub fn daemon_with(environment: &[(&str, &str)]) -> Daemon {
    Daemon::start_with(env!("CARGO_BIN_EXE_muster-daemon"), environment)
}

pub fn daemon_holding(descriptor: i32) -> Daemon {
    Daemon::start_holding(env!("CARGO_BIN_EXE_muster-daemon"), descriptor)
}

/// What a file a pane was asked to write says, once it says something.
pub fn written(path: &Path) -> String {
    until_file(path, &format!("a pane to write {}", path.display()));
    // A redirect creates the file before the command has finished writing it, so wait for the
    // line to be complete.
    until_some(&format!("{} to end in a newline", path.display()), || {
        std::fs::read_to_string(path).ok().filter(|text| text.ends_with('\n'))
    })
}

/// A path as the kernel names it, so `/tmp` and `/private/tmp` compare equal on macOS.
pub fn canonical(path: &Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// What `ps` says a process is, or nothing once it has gone and been reaped.
pub fn process_state(pid: &str) -> String {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=,args=", "-p", pid.trim()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A command that puts its terminal in raw mode, writes `query` (a printf format), and saves
/// whatever comes back on its input to `out` - which is how a test sees the daemon answer a
/// program's query as the program would.
///
/// The read waits for the answer however long it takes, never on a clock of its own: a loaded
/// machine can take longer than any fixed window, and an empty file from a window that elapsed
/// would fail the test as though the daemon had not answered. One that never answers still
/// fails, at the test's patience.
pub fn answer_to(query: &str, out: &Path) -> String {
    format!(
        "stty raw -echo min 1 time 0; printf '{query}'; dd bs=4096 count=1 of={} 2>/dev/null",
        out.display()
    )
}

/// What a file holds once something has been written to it.
pub fn bytes_in(path: &Path) -> Vec<u8> {
    until_some(&format!("something written to {}", path.display()), || {
        std::fs::read(path).ok().filter(|bytes| !bytes.is_empty())
    })
}

pub use muster_harness::Stream;
use proto::stream_message::Message as Streamed;

/// What a bridge's surface would show: a terminal fed every replay and every byte of output
/// its stream carries, and what else the stream said.
pub struct Surface {
    pub terminal: muster_vt::Terminal,
    /// Output fed and not yet acknowledged.
    pub unacknowledged: u64,
    /// Output fed since attaching.
    pub output: u64,
    pub attached: Option<u64>,
    pub replays: usize,
    pub behind: usize,
    pub detached: Option<proto::DetachReason>,
    pub refused: Option<String>,
    /// The daemon hung up.
    pub ended: bool,
}

impl Surface {
    pub fn new(cols: u16, rows: u16) -> Surface {
        let mut options = muster_vt::TerminalOptions::new(cols, rows);
        options.scrollback_bytes = Some(64 << 20);
        Surface {
            terminal: muster_vt::Terminal::with_options(options).expect("a terminal"),
            unacknowledged: 0,
            output: 0,
            attached: None,
            replays: 0,
            behind: 0,
            detached: None,
            refused: None,
            ended: false,
        }
    }

    pub fn take(&mut self, message: Option<Streamed>) {
        match message {
            Some(Streamed::Attached(attached)) => self.attached = Some(attached.offset),
            Some(Streamed::Replay(bytes)) => {
                self.replays += 1;
                self.terminal.write(&bytes);
            }
            Some(Streamed::Output(bytes)) => {
                self.unacknowledged += bytes.len() as u64;
                self.output += bytes.len() as u64;
                self.terminal.write(&bytes);
            }
            Some(Streamed::Behind(_)) => self.behind += 1,
            Some(Streamed::Detached(detached)) => self.detached = Some(detached.reason()),
            Some(Streamed::Refused(refused)) => self.refused = Some(refused.reason),
            None => self.ended = true,
        }
    }

    /// Everything the surface holds, history first, as text.
    pub fn text(&self) -> String {
        self.terminal.screen_text(0, u32::MAX)
    }

    /// What the surface shows, without its history: cheap enough to check after every message.
    pub fn screen(&self) -> String {
        self.terminal.text(0, self.terminal.rows().saturating_sub(1))
    }

    /// Takes messages until `done` says so, acknowledging output as it goes when `credit`.
    pub fn follow(
        &mut self,
        stream: &mut Stream,
        what: &str,
        credit: bool,
        mut done: impl FnMut(&Surface) -> bool,
    ) {
        let deadline = std::time::Instant::now() + muster_harness::PATIENCE;
        while !done(self) {
            if credit && self.unacknowledged > 0 {
                stream.credit(self.unacknowledged);
                self.unacknowledged = 0;
            }
            assert!(
                !self.ended,
                "{what}: the daemon hung up; the surface shows {:?}",
                self.screen()
            );
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !left.is_zero(),
                "{what}: not within the suite's patience; {:?}",
                self.screen()
            );
            if let Some(message) = stream.next_within(left) {
                self.take(message);
            }
        }
    }
}

/// A stream attached to `name`.
pub fn attached(daemon: &Daemon, name: &str, takeover: bool) -> Stream {
    let mut stream = Stream::connect(daemon.socket_path());
    stream.attach(name, None, takeover);
    stream
}
