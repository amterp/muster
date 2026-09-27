//! A real muster-daemon, owned by one test.
//!
//! Built from the same commit as the test, never downloaded or pinned: the caller passes the
//! binary, and the daemon's own tests pass `CARGO_BIN_EXE_muster-daemon`, which cargo builds
//! fresh for them. Isolated by giving it a scratch root: its own socket, its own HOME, its own
//! log. Nothing here can reach a daemon somebody is working in.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use muster_daemon_proto::{self as proto, request::Service, session_request};

use crate::control::Control;
use crate::relay::{Holding, Relay};
use crate::relay_daemon::DaemonPump;
use crate::until::{PATIENCE, until_within};

/// How long a spawned daemon may take to answer its first request.
///
/// herdr's figure, measured when the suite chose to run a real daemon per test
/// (`docs/testing.md`): 25 ms to spawn and answer. Daemon-backed tests stay in the default gate
/// only while a daemon costs about that, so muster-daemon is held to it too.
pub const FIRST_ANSWER_BUDGET: Duration = Duration::from_millis(25);

/// A socket path must fit `sockaddr_un.sun_path`, 104 bytes on macOS, so the root stays short.
const ROOT: &str = "/tmp/muster-test";

/// What a pane runs unless a test sets a shell: a `/bin/sh`, so nobody's dotfiles play a part.
const SHELL: &str = "/bin/sh";

static NEXT: AtomicU32 = AtomicU32::new(0);

/// One daemon, one test. Killed on drop, including when the test panics, and its root removed.
#[derive(Debug)]
pub struct Daemon {
    binary: PathBuf,
    root: PathBuf,
    socket_path: PathBuf,
    process: Option<Child>,
    started_in: Duration,
}

impl Daemon {
    /// Starts the daemon at `binary` and waits for it to answer a snapshot.
    pub fn start(binary: impl AsRef<Path>) -> Daemon {
        Daemon::start_with(binary, &[])
    }

    /// The same, with more in the daemon's environment - which every pane's starts from.
    pub fn start_with(binary: impl AsRef<Path>, environment: &[(&str, &str)]) -> Daemon {
        let root = PathBuf::from(ROOT).join(format!(
            "d{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // A leftover root from a killed run would hand this test someone else's socket.
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap_or_else(|error| {
            panic!("could not create the harness root at {}: {error}", root.display())
        });
        let mut daemon = Daemon {
            binary: binary.as_ref().to_path_buf(),
            socket_path: root.join("daemon.sock"),
            root,
            process: None,
            started_in: Duration::ZERO,
        };
        daemon.process = Some(daemon.spawn(environment));
        daemon.started_in = daemon.wait_until_answering();
        daemon
    }

    /// Starts another daemon process on this daemon's socket, and hands it back unwaited: the
    /// one a test about a second daemon on a claimed socket watches exit.
    pub fn spawn_another(&self) -> Child {
        self.spawn(&[])
    }

    fn spawn(&self, environment: &[(&str, &str)]) -> Child {
        let log = std::fs::File::options()
            .create(true)
            .append(true)
            .open(self.root.join("stderr.log"))
            .expect("could not open the harness's stderr log");
        Command::new(&self.binary)
            .arg("--socket")
            .arg(&self.socket_path)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", self.root.join("home"))
            .env("SHELL", SHELL)
            .env("MUSTER_LOG_FILE", self.root.join("daemon.log"))
            .envs(environment.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap_or_else(|error| {
                panic!(
                    "could not run muster-daemon at {}: {error}\n  Impact: this test has no \
                     daemon.\n  Fix: pass env!(\"CARGO_BIN_EXE_muster-daemon\") from a test in \
                     the muster-daemon package, which cargo builds before running it.",
                    self.binary.display()
                )
            })
    }

    /// Dials until the daemon answers a snapshot, and says how long that took from the spawn.
    fn wait_until_answering(&mut self) -> Duration {
        let spawned = Instant::now();
        let deadline = spawned + PATIENCE;
        while Instant::now() < deadline {
            if let Ok(mut control) = Control::try_connect(&self.socket_path) {
                control.ask(Service::Session(proto::SessionRequest {
                    request: Some(session_request::Request::Snapshot(session_request::Snapshot {})),
                }));
                return spawned.elapsed();
            }
            if let Some(process) = &mut self.process
                && let Ok(Some(status)) = process.try_wait()
            {
                panic!(
                    "muster-daemon exited with {status} before answering.\n  Impact: this test \
                     has no daemon.\n  Check {} and {}.",
                    self.root.join("stderr.log").display(),
                    self.root.join("daemon.log").display()
                );
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!(
            "muster-daemon did not answer on {} within {PATIENCE:?}.\n  Impact: this test has no \
             daemon.\n  Check {} for why.",
            self.socket_path.display(),
            self.root.join("daemon.log").display()
        );
    }

    /// From spawning the process to the answer to its first request.
    pub fn started_in(&self) -> Duration {
        self.started_in
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// This daemon's scratch directory, removed with it.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A new control connection, welcomed.
    pub fn connect(&self) -> Control {
        Control::connect(&self.socket_path)
    }

    /// A relay in front of this daemon that never delivers the answer to any request `withheld`
    /// picks out. The daemon still does the work, and every event still arrives.
    pub fn withholding_answers_where(
        &self,
        withheld: impl Fn(&proto::Request) -> bool + Send + Sync + 'static,
    ) -> Relay {
        let pump = DaemonPump::new(Arc::new(withheld), Holding::Forever);
        Relay::start(&self.root, &self.socket_path, Arc::new(pump))
    }

    /// The same, delivering those answers `delay` late.
    pub fn delaying_answers_where(
        &self,
        withheld: impl Fn(&proto::Request) -> bool + Send + Sync + 'static,
        delay: Duration,
    ) -> Relay {
        let pump = DaemonPump::new(Arc::new(withheld), Holding::For(delay));
        Relay::start(&self.root, &self.socket_path, Arc::new(pump))
    }

    /// Waits for the daemon to exit by itself, as a `stop` or a signal asks it to.
    pub fn wait_for_exit(&mut self) -> ExitStatus {
        let process = self.process.as_mut().expect("the daemon is running");
        let mut status = None;
        until_within(
            "the daemon to exit",
            PATIENCE,
            || {
                status = process.try_wait().ok().flatten();
                status.is_some()
            },
            (),
        );
        self.process = None;
        status.expect("the wait ended with a status")
    }

    /// The daemon's process id, for a test that signals it.
    pub fn pid(&self) -> u32 {
        self.process.as_ref().expect("the daemon is running").id()
    }

    /// Ends the daemon abruptly, the way a crash does.
    pub fn kill(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.kill();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
