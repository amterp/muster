//! Spike: can tests against a real herdr live in the default gate?
//!
//! `docs/testing.md` says the gate runs offline against a fake, and that a drifted fake is
//! Muster's top false-green risk. Those pull in opposite directions, and the mirror
//! (`a_26DAm1Zt0`) is the first thing where it matters. This measures the real option
//! rather than arguing about it: how long a daemon costs, how reliable it is across
//! repeats, and which failures can be provoked on demand.
//!
//! Every test here is `#[ignore]`d, so `./dev` does not run it. Run it deliberately:
//!
//!     cargo test -p muster-herdr --test real_daemon_spike -- --ignored --nocapture --test-threads=1
//!
//! This file is evidence for a decision, not coverage. It gets deleted or promoted once
//! the decision is made; leaving it ignored forever is how a suite grows dead weight.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A herdr daemon this test owns end to end.
///
/// Ported from `tools/herdr-probe/herdrprobe/daemon.py`, which is the arrangement herdr's
/// own integration tests also use: point the XDG variables at a scratch root and the
/// daemon gets its own socket, its own session, and no view of the developer's real one.
struct Daemon {
    root: PathBuf,
    socket: PathBuf,
    process: Option<Child>,
}

impl Daemon {
    /// The root stays short on purpose: a Unix socket path must fit `sun_path`, 104 bytes
    /// on macOS. The socket lives inside the config dir, and cargo's own temp directories
    /// are already too long on their own.
    fn new(slot: usize) -> Daemon {
        let root = PathBuf::from(format!("/tmp/muster-spike-{slot}"));
        let socket = root.join("config/herdr/herdr.sock");
        assert!(
            socket.as_os_str().len() < 100,
            "socket path {} is too long for sun_path",
            socket.display()
        );
        Daemon { root, socket, process: None }
    }

    fn prepare(&self) {
        let _ = std::fs::remove_dir_all(&self.root);
        for dir in ["config/herdr", "state", "home", "data", "cache"] {
            std::fs::create_dir_all(self.root.join(dir)).expect("scratch dirs");
        }
        // Same config the probe records against: a non-login /bin/sh, so the developer's
        // dotfiles play no part, and no update checks, so a run works offline.
        std::fs::write(
            self.root.join("config/herdr/config.toml"),
            "[terminal]\ndefault_shell = \"/bin/sh\"\nshell_mode = \"non_login\"\n\
             new_cwd = \"current\"\n\n[update]\nversion_check = false\nmanifest_check = false\n",
        )
        .expect("config.toml");
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("herdr");
        command.args(args);
        command.env_remove("HERDR_SOCKET_PATH");
        command.env_remove("HERDR_CLIENT_SOCKET_PATH");
        command.env_remove("HERDR_SESSION");
        command.env("HOME", self.root.join("home"));
        command.env("XDG_CONFIG_HOME", self.root.join("config"));
        command.env("XDG_STATE_HOME", self.root.join("state"));
        command.env("XDG_DATA_HOME", self.root.join("data"));
        command.env("XDG_CACHE_HOME", self.root.join("cache"));
        command.env("TERM", "xterm-256color");
        command
    }

    /// Returns how long the daemon took to answer its first request.
    fn start(&mut self) -> Duration {
        let started = Instant::now();
        let log = std::fs::File::create(self.root.join("server.log")).expect("server log");
        self.process = Some(
            self.command(&["server"])
                .stdin(Stdio::null())
                .stdout(Stdio::from(log.try_clone().expect("log handle")))
                .stderr(Stdio::from(log))
                .spawn()
                .expect("herdr server should spawn - is herdr on PATH?"),
        );

        // Event-driven rather than a sleep: poll until it answers, which is what herdr's
        // own suite does (`wait_for_socket`).
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if self.socket.exists() && self.request("ping", "{}").is_ok() {
                return started.elapsed();
            }
            if let Some(process) = self.process.as_mut()
                && let Ok(Some(status)) = process.try_wait()
            {
                panic!(
                    "herdr server exited with {status} before accepting a connection. \
                     See {}",
                    self.root.join("server.log").display()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("herdr server never answered on {}", self.socket.display());
    }

    /// One request, one connection - which is all herdr allows.
    fn request(&self, method: &str, params: &str) -> Result<String, String> {
        let mut stream = UnixStream::connect(&self.socket).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
        let line = format!("{{\"id\":\"spike\",\"method\":\"{method}\",\"params\":{params}}}\n");
        stream.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).map_err(|e| e.to_string())?;
        if answer.is_empty() {
            Err("daemon closed without answering".to_string())
        } else {
            Ok(answer)
        }
    }

    /// A held-open subscription, returned as the reader so a test can watch it die.
    fn subscribe(&self, types: &[&str]) -> BufReader<UnixStream> {
        let subscriptions: Vec<String> =
            types.iter().map(|t| format!("{{\"type\":\"{t}\"}}")).collect();
        let mut stream = UnixStream::connect(&self.socket).expect("subscribe connect");
        let line = format!(
            "{{\"id\":\"sub\",\"method\":\"events.subscribe\",\"params\":{{\"subscriptions\":[{}]}}}}\n",
            subscriptions.join(",")
        );
        stream.write_all(line.as_bytes()).expect("subscribe write");
        let mut reader = BufReader::new(stream);
        let mut ack = String::new();
        reader.read_line(&mut ack).expect("subscription ack");
        assert!(ack.contains("subscription_started"), "unexpected ack: {ack}");
        reader
    }

    fn stop(&mut self) {
        let _ =
            self.command(&["server", "stop"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        if let Some(mut process) = self.process.take() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if matches!(process.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = process.kill();
            let _ = process.wait();
        }
    }

    /// The disconnect case, without any proxy: SIGKILL is a daemon that stops mid-stream.
    fn kill_hard(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // A leaked daemon outlives the test run and holds a socket the next one wants.
        self.stop();
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[test]
#[ignore = "spike: needs a real herdr on PATH"]
fn what_a_daemon_costs() {
    let mut spawns = Vec::new();
    let mut stops = Vec::new();
    for round in 0..5 {
        let mut daemon = Daemon::new(round);
        daemon.prepare();
        spawns.push(daemon.start());
        let stopping = Instant::now();
        daemon.stop();
        stops.push(stopping.elapsed());
    }
    let mean = |values: &[Duration]| {
        values.iter().sum::<Duration>() / u32::try_from(values.len()).unwrap_or(1)
    };
    println!(
        "\nspawn-to-first-answer: {:?}\n  mean {:.0} ms\nstop: mean {:.0} ms\n\
         one daemon per test would cost {:.0} ms of setup and teardown each.",
        spawns.iter().map(|d| format!("{:.0}ms", millis(*d))).collect::<Vec<_>>(),
        millis(mean(&spawns)),
        millis(mean(&stops)),
        millis(mean(&spawns) + mean(&stops)),
    );
}

#[test]
#[ignore = "spike: needs a real herdr on PATH"]
fn what_the_operations_a_mirror_makes_cost() {
    let mut daemon = Daemon::new(10);
    daemon.prepare();
    let spawn = daemon.start();

    let timed = |label: &str, run: &dyn Fn()| {
        let started = Instant::now();
        run();
        let elapsed = started.elapsed();
        println!("  {label:32} {:>7.1} ms", millis(elapsed));
        elapsed
    };

    println!("\nagainst one already-running daemon:");
    timed("workspace.create", &|| {
        daemon
            .request("workspace.create", "{\"cwd\":\"/tmp\",\"focus\":true,\"label\":null}")
            .expect("workspace.create");
    });
    let snapshot = timed("session.snapshot", &|| {
        daemon.request("session.snapshot", "{}").expect("session.snapshot");
    });
    timed("pane.split", &|| {
        daemon
            .request("pane.split", "{\"direction\":\"right\",\"target_pane_id\":\"w1:p1\"}")
            .expect("pane.split");
    });

    // The bootstrap a mirror actually performs, end to end.
    let bootstrap = Instant::now();
    daemon.request("session.snapshot", "{}").expect("snapshot");
    let mut events = daemon.subscribe(&["pane.created", "tab.created", "workspace.created"]);
    let mut replayed = String::new();
    events.read_line(&mut replayed).expect("replayed event");
    let bootstrap = bootstrap.elapsed();

    println!(
        "  {:32} {:>7.1} ms  (snapshot + subscribe + first replayed event)",
        "mirror bootstrap",
        millis(bootstrap)
    );
    println!(
        "\nso a test sharing one daemon pays about {:.0} ms, and one spawning its own pays \
         about {:.0} ms more.",
        millis(snapshot + bootstrap),
        millis(spawn)
    );
}

#[test]
#[ignore = "spike: needs a real herdr on PATH"]
fn a_dying_daemon_is_the_disconnect_case() {
    // The question this settles: does testing "the control plane dropped" need an
    // injecting proxy, or is killing a real daemon enough? If it is enough, the most
    // important failure case needs no fake at all.
    let mut daemon = Daemon::new(11);
    daemon.prepare();
    daemon.start();
    daemon.request("workspace.create", "{\"cwd\":\"/tmp\",\"focus\":true,\"label\":null}").unwrap();

    let mut events = daemon.subscribe(&["pane.created", "pane.closed"]);
    let mut replayed = String::new();
    events.read_line(&mut replayed).expect("a replayed event before the kill");

    let killed = Instant::now();
    daemon.kill_hard();

    // A blocked reader on a dead daemon: EOF, or an error, but never a hang.
    let mut line = String::new();
    let read = events.read_line(&mut line);
    let noticed = killed.elapsed();

    println!(
        "\nSIGKILL to EOF on a held-open subscription: {:.1} ms, read returned {:?}, \
         bytes {:?}",
        millis(noticed),
        read.as_ref().map(|n| *n),
        line
    );
    assert!(
        matches!(read, Ok(0)) || read.is_err(),
        "a killed daemon should end the stream, got {read:?} with {line:?}"
    );
    assert!(noticed < Duration::from_secs(2), "took {noticed:?} to notice a dead daemon");
    println!("=> the disconnect case needs no proxy and no fake: kill the daemon.");
}

#[test]
#[ignore = "spike: needs a real herdr on PATH"]
fn what_a_real_daemon_will_not_do_on_cue() {
    // The other half of the answer. These are the failures a mirror has to survive and
    // that a real daemon will not produce when asked, so whatever covers them is Muster's
    // own code sitting in the wire path.
    println!(
        "\nnot provokable against a real daemon:\n\
         \x20 - a JSON line truncated mid-frame (needs a writer that stops mid-line)\n\
         \x20 - an answer that arrives too slowly (no config knob; herdr's timings are \
         hardcoded consts)\n\
         \x20 - events delivered out of order (herdr orders its own hub)\n\
         all three are byte-stream shapes rather than daemon behaviors, which is the \
         argument for a parser that takes a Read instead of a socket."
    );
}
