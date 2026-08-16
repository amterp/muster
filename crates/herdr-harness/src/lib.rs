//! A real herdr daemon, owned by one test.
//!
//! Muster's suite does not fake its backend. A fake is Muster's own guess at what herdr
//! does, and a wrong guess passes - which is the one failure a test suite must not have
//! (`docs/testing.md`). So the tests that need a daemon start the pinned one, drive it, and
//! kill it.
//!
//! Isolated through the environment rather than through a flag: herdr resolves its config
//! directory from XDG and keeps the control socket inside it, so a scratch root yields a
//! daemon with its own socket, its own session, and no view of the developer's. Nothing
//! here can touch a session someone is working in.
//!
//! The Rust twin of `tools/herdr-probe/herdrprobe/daemon.py`, which does the same job for
//! recording the corpus. Two of them because they are used from two languages at two
//! moments - the probe records what herdr does, this drives what Muster does about it -
//! and the facts they encode about isolation are kept the same on purpose.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use muster_herdr::{HerdrBackend, HerdrClient, PaneEnvironment};
use serde_json::{Value, json};

/// A Unix socket path must fit `sockaddr_un.sun_path`: 104 bytes on macOS, 108 on Linux.
/// The socket lives inside the config dir, so the root has to stay short - a per-session
/// scratch directory is already too long on its own, and the failure is herdr exiting
/// immediately with an opaque `InvalidInput`.
const ROOT: &str = "/tmp/muster-test";
const SUN_PATH_MAX: usize = 100;

/// Deterministic and offline. The shell is pinned to a non-login `/bin/sh` so a
/// developer's dotfiles play no part in what a test sees - a login zsh under a scratch
/// HOME exits nonzero, which closes the pane, then the workspace, then the whole headless
/// server.
const CONFIG_TOML: &str = "\
[terminal]
default_shell = \"/bin/sh\"
shell_mode = \"non_login\"
new_cwd = \"current\"

[update]
version_check = false
manifest_check = false
";

static NEXT: AtomicU32 = AtomicU32::new(0);

/// One daemon, one test.
///
/// Killed on drop, including when the test panics, because a leaked `herdr server` holds
/// its socket and the next run inherits a session it did not create.
#[derive(Debug)]
pub struct Daemon {
    root: PathBuf,
    socket_path: PathBuf,
    process: Option<Child>,
}

impl Daemon {
    /// Starts a daemon and waits for it to answer.
    ///
    /// Panics rather than returning an error: every caller is a test whose only response
    /// would be to fail, and a daemon that will not start is an environment problem rather
    /// than a behavior under test. The message says which, because the two look identical
    /// from a failing assertion.
    pub fn start() -> Daemon {
        let binary = binary();
        let root = PathBuf::from(ROOT).join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let config_dir = root.join("config/herdr");
        let socket_path = config_dir.join("herdr.sock");
        assert!(
            socket_path.as_os_str().len() <= SUN_PATH_MAX,
            "the harness root yields a {}-byte socket path, over the {SUN_PATH_MAX}-byte \
             sockaddr_un limit.\n  Impact: herdr exits immediately with InvalidInput and \
             every daemon-backed test fails at startup.\n  Fix: shorten ROOT in \
             crates/herdr-harness.",
            socket_path.as_os_str().len()
        );

        // A leftover root from a killed run would hand this test someone else's session.
        let _ = std::fs::remove_dir_all(&root);
        for directory in ["config/herdr", "state", "home", "data", "cache"] {
            std::fs::create_dir_all(root.join(directory)).unwrap_or_else(|error| {
                panic!("could not create the harness root at {}: {error}", root.display())
            });
        }
        std::fs::write(config_dir.join("config.toml"), CONFIG_TOML)
            .expect("could not write the harness config");

        let log = std::fs::File::create(root.join("server.log"))
            .expect("could not open the harness server log");
        let process = Command::new(&binary)
            .arg("server")
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("TERM", "xterm-256color")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().expect("could not share the harness log")))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap_or_else(|error| {
                panic!(
                    "could not run the pinned herdr at {binary}: {error}\n  Impact: every \
                     daemon-backed test fails.\n  Fix: run ./dev -t, which downloads the \
                     binary named by deps/herdr.pin, or point MUSTER_HERDR at one."
                )
            });

        let mut daemon = Daemon { root: root.clone(), socket_path, process: Some(process) };
        daemon.wait_until_answering();
        daemon
    }

    fn wait_until_answering(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if self.socket_path.exists() && self.client().request("ping", &json!({})).is_ok() {
                return;
            }
            if let Some(process) = &mut self.process
                && let Ok(Some(status)) = process.try_wait()
            {
                panic!(
                    "herdr exited with {status} before accepting a connection.\n  Impact: \
                     this test has no daemon.\n  Check {} - a socket collision, an \
                     unreadable config, or a herdr whose wire has moved are the usual \
                     causes.",
                    self.root.join("server.log").display()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "herdr did not answer on {} within 20s.\n  Impact: this test has no daemon.\n  \
             Check {} for startup errors.",
            self.socket_path.display(),
            self.root.join("server.log").display()
        );
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Writes a Muster config file naming this daemon, and returns its path.
    ///
    /// How a test points the core at a scratch daemon, and the only way there is. Muster runs
    /// its own herdr on a socket of its own and does not read `HERDR_SOCKET_PATH` - a window
    /// that silently joined whatever the environment pointed at would be back to attaching a
    /// daemon of unknown version. Naming a `socket` in the config file is the deliberate way
    /// to ask for a particular one, so a test asks the way a person would.
    ///
    /// It also means a test needs no environment mutation, which in a process that is also
    /// running threads is worth more than the line it saves.
    pub fn muster_config(&self) -> PathBuf {
        self.muster_config_with("")
    }

    /// The same file with more in it, for a test about a setting rather than about a daemon.
    ///
    /// Written as text rather than built from a config type, because what is under test is
    /// what a person types: a setting reached by constructing the parsed form would pass
    /// while the file that names it was being refused.
    ///
    /// The extra goes *first*, which is not a style choice. In TOML every bare key after a
    /// table header belongs to that table, so `option_as_alt` written below `[[daemon]]`
    /// becomes a key of the daemon block and the file is refused for naming something a
    /// daemon does not have. A refused config is not a failed test either - Muster falls back
    /// to finding a daemon for itself, so the run quietly attaches to whatever herdr the
    /// developer has open and fails somewhere unrecognizable.
    pub fn muster_config_with(&self, extra: &str) -> PathBuf {
        let path = self.root.join("muster.toml");
        let preamble = if extra.is_empty() { String::new() } else { format!("{extra}\n") };
        let contents = format!(
            "{preamble}[[daemon]]\nid = \"local\"\nsocket = {:?}\n",
            self.socket_path.to_string_lossy()
        );
        std::fs::write(&path, contents).unwrap_or_else(|error| {
            panic!("could not write the harness's Muster config at {}: {error}", path.display())
        });
        path
    }

    pub fn client(&self) -> HerdrClient {
        // Longer than the client's own default, which is tuned for the input path where a
        // wedged daemon must not take the keyboard with it. A test would rather wait than
        // flake on a machine under load.
        HerdrClient::with_timeout(
            self.socket_path.to_string_lossy().into_owned(),
            Duration::from_secs(5),
        )
    }

    /// The same daemon, as the thing a `BackendIntent` is submitted to.
    ///
    /// The pane environment is empty, which is what a daemon nobody redirected gets: the
    /// harness writes this daemon's herdr config itself rather than deriving one, so a pane
    /// on it has nothing to be pointed back at.
    pub fn backend(&self) -> HerdrBackend {
        HerdrBackend::new(self.client(), PaneEnvironment::none())
    }

    /// Sends a request and unwraps it, naming the method when it fails.
    ///
    /// Setup, not assertion: `daemon.call("workspace.create", ..)` failing means the test
    /// never got to what it was about, and the panic should say so rather than surfacing
    /// three lines later as an empty snapshot.
    pub fn call(&self, method: &str, params: &Value) -> Value {
        self.client().request(method, params).unwrap_or_else(|failure| {
            panic!(
                "herdr refused {method}: {failure}\n  Impact: this test's setup did not \
                    happen, so what it asserts below was never exercised."
            )
        })
    }

    /// Ends the daemon abruptly, the way a crash or a lost machine does.
    ///
    /// The reason a real daemon can cover what a fake was going to: a hang-up mid-stream is
    /// something reality produces on demand, so long as the test can ask for it.
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
        // The log outlives a passing test only as long as the test does. A failing one
        // has already printed the path.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The herdr the suite is judged against.
///
/// From the environment, never from PATH. `./dev` downloads the binary named by
/// `deps/herdr.pin`, verifies its checksum, and passes the path down, so a suite that
/// passed did so against the daemon the corpus was recorded with - while the herdr on the
/// developer's PATH stays whatever version their own work wants.
///
/// Public because a test may have to hand this to a subprocess of its own. `muster-bridge`
/// runs `herdr terminal session control` off PATH, which is right in production and would
/// otherwise reach for a version nobody verified here, so a test that spawns one puts this
/// binary's directory on the PATH it hands over.
pub fn binary() -> String {
    std::env::var("MUSTER_HERDR").unwrap_or_else(|_| {
        panic!(
            "MUSTER_HERDR is not set.\n  Impact: this test needs a real daemon and has no \
             binary to start.\n  Fix: run the suite through ./dev -t, which downloads the \
             pinned herdr and passes it down. Setting it by hand skips the checksum and \
             version check."
        )
    })
}
