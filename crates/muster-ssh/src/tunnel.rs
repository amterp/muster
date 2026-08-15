use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;

/// What one master forwards, and how it is reached.
#[derive(Debug, Clone)]
pub struct Forward {
    /// An ssh destination, spelled however ssh accepts it: a host, `user@host`, or an alias
    /// out of the user's own config.
    pub host: String,
    /// Handed to ssh verbatim, after Muster's own options. Connection details belong in
    /// `~/.ssh/config`; this is the escape hatch for what a host alias cannot cover.
    pub options: Vec<String>,
    /// Where the master's control socket goes, so that a bridge can run a command through
    /// this connection rather than opening one of its own.
    pub control_path: String,
    /// The path on this machine that will answer as though it were the daemon's own.
    pub local_socket: String,
    /// The daemon's socket, over there.
    pub remote_socket: String,
}

/// The longest a unix socket path can be, near enough.
///
/// `sockaddr_un.sun_path` is 104 bytes on macOS and 108 on Linux, and the shorter one is what
/// has to fit. Checked here rather than discovered at connect time, because the failure is an
/// `EINVAL` from a bind nobody can trace back to a name in a config file.
const SUN_PATH_LIMIT: usize = 100;

/// What ssh is told, before whatever the user adds.
///
/// Muster's own options come first, and ssh takes the first value it is given for any
/// setting, so these are not overridable. That is deliberate: they are what make a broken
/// tunnel fail loudly instead of hanging a window, and the escape hatch exists for
/// connection details rather than for how Muster supervises its own child.
pub fn master_arguments(forward: &Forward) -> Vec<String> {
    let mut arguments: Vec<String> = vec![
        // Forward and wait. There is no remote command: the socket is the whole point.
        "-N".to_string(),
        // The master, so that every pane's bridge rides this one connection instead of
        // authenticating again. Fifteen panes on a devenv is fifteen `ssh` invocations
        // otherwise, each paying a full handshake.
        "-M".to_string(),
        "-S".to_string(),
        forward.control_path.clone(),
        "-L".to_string(),
        format!("{}:{}", forward.local_socket, forward.remote_socket),
        // A GUI app has no terminal to prompt on, so a connection that wants a password must
        // fail rather than wait for an answer that cannot arrive.
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        // Without this a failed forward leaves ssh connected and the local socket absent, so
        // every request fails with "no such file" and nothing says why.
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        // How a dropped VPN gets noticed rather than black-holed. A TCP connection nobody
        // reset stays open forever, and the pane channels riding it would sit silent while
        // the window claimed to be connected.
        "-o".to_string(),
        "ServerAliveInterval=15".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
    ];
    arguments.extend(forward.options.iter().cloned());
    arguments.push(forward.host.clone());
    arguments
}

/// A live ssh master, and the local path it forwards.
///
/// Dropping it takes the connection down, which takes every pane channel riding it down too -
/// so it is held for exactly as long as the daemon is attached.
#[derive(Debug)]
pub struct Tunnel {
    forward: Forward,
    child: Arc<Mutex<Child>>,
    stopping: Arc<AtomicBool>,
}

impl Tunnel {
    /// How long to wait for the forwarded socket to appear before giving up.
    ///
    /// Long enough for a handshake over a slow link, short enough that a window does not sit
    /// blank while somebody wonders whether it is broken.
    const READY_TIMEOUT: Duration = Duration::from_secs(20);

    /// Opens the connection and waits for the socket to exist.
    ///
    /// Waited for rather than assumed, because everything above takes the path on trust: a
    /// caller handed a path before ssh has bound it would report a daemon that is not running
    /// when the truth is a handshake that had not finished.
    ///
    /// What is *not* verified here is that anything answers on the far end. That is the
    /// adapter's first request, and it already says what a silent daemon means - checking it
    /// here would put herdr's vocabulary in a crate that has none.
    pub fn open(forward: Forward) -> Result<Tunnel, String> {
        if forward.local_socket.len() > SUN_PATH_LIMIT {
            return Err(format!(
                "the local end of this daemon's tunnel would be {} bytes of path, and a unix \
                 socket has about {SUN_PATH_LIMIT} to spend ({}). Nothing was connected, so \
                 that daemon's panes are absent from the window. Shorten the daemon's name in \
                 the config file, or point TMPDIR somewhere shorter.",
                forward.local_socket.len(),
                forward.local_socket,
            ));
        }
        // A path left behind by a previous run binds nothing and refuses everything, and ssh
        // will not replace it. Removing it is safe because the name carries this process's id.
        let _ = std::fs::remove_file(&forward.local_socket);

        let child = spawn(&forward)?;
        let tunnel = Tunnel {
            forward,
            child: Arc::new(Mutex::new(child)),
            stopping: Arc::new(AtomicBool::new(false)),
        };
        tunnel.wait_for_socket()?;
        tunnel.supervise();
        Ok(tunnel)
    }

    /// The path on this machine that answers as the daemon's own socket.
    pub fn local_socket_path(&self) -> &str {
        &self.forward.local_socket
    }

    /// The master's control socket, for a command that wants to ride this connection.
    pub fn control_path(&self) -> &str {
        &self.forward.control_path
    }

    pub fn host(&self) -> &str {
        &self.forward.host
    }

    fn wait_for_socket(&self) -> Result<(), String> {
        let deadline = Instant::now() + Tunnel::READY_TIMEOUT;
        while Instant::now() < deadline {
            if Path::new(&self.forward.local_socket).exists() {
                log::info(
                    "tunnel.open",
                    fields! {
                        "host" => self.forward.host.clone(),
                        "local" => self.forward.local_socket.clone(),
                        "remote" => self.forward.remote_socket.clone(),
                    },
                );
                return Ok(());
            }
            // Exited early means the forward was refused, and ssh has already said why on the
            // stderr this process inherits.
            if let Ok(mut child) = self.child.lock()
                && let Ok(Some(status)) = child.try_wait()
            {
                return Err(format!(
                    "ssh to {} ended before it forwarded anything ({status}). That daemon's \
                     panes are absent from the window and nothing else is affected. Its own \
                     message is above this; the usual causes are a host this machine cannot \
                     reach, a key that needs a passphrase - Muster runs ssh in batch mode and \
                     cannot answer a prompt - or a remote socket path that does not exist.",
                    self.forward.host,
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Err(format!(
            "ssh to {} did not forward a socket within {} seconds, so that daemon's panes are \
             absent from the window. It is still connecting, or the far end is not answering; \
             check that the host is reachable and that a daemon is listening at {}.",
            self.forward.host,
            Tunnel::READY_TIMEOUT.as_secs(),
            self.forward.remote_socket,
        ))
    }

    /// Brings the connection back when it dies, without telling anyone above.
    ///
    /// The local path is the same across a restart, so nothing that holds it needs to learn
    /// that ssh went away - the adapter's own reconnect finds the socket answering again and
    /// resyncs. That is the payoff of forwarding a socket rather than running a command per
    /// request: recovery is a mechanism that already exists.
    ///
    /// Backoff rather than a tight loop, because the common reason a tunnel dies is a network
    /// that is still gone, and a lid that has been shut overnight should not have spent the
    /// night spawning processes.
    fn supervise(&self) {
        let forward = self.forward.clone();
        let child = Arc::clone(&self.child);
        let stopping = Arc::clone(&self.stopping);
        std::thread::spawn(move || {
            const BACKOFF: [Duration; 4] = [
                Duration::from_millis(50),
                Duration::from_millis(200),
                Duration::from_millis(500),
                Duration::from_secs(1),
            ];
            let mut attempt = 0;
            while !stopping.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(250));
                if stopping.load(Ordering::Relaxed) {
                    return;
                }
                let alive = child
                    .lock()
                    .ok()
                    .and_then(|mut held| held.try_wait().ok())
                    .is_some_and(|exited| exited.is_none());
                if alive {
                    attempt = 0;
                    continue;
                }
                log::warn(
                    "tunnel.down",
                    fields! {
                        "host" => forward.host.clone(),
                        "impact" => "every pane on this daemon is rendering what it last \
                                     showed, and its agent states are a guess about the \
                                     present",
                        "check" => "whether the host is reachable - the connection is being \
                                    retried and recovers on its own once it is",
                    },
                );
                std::thread::sleep(BACKOFF[attempt.min(BACKOFF.len() - 1)]);
                attempt += 1;
                if stopping.load(Ordering::Relaxed) {
                    return;
                }
                let _ = std::fs::remove_file(&forward.local_socket);
                match spawn(&forward) {
                    Ok(fresh) => {
                        if let Ok(mut held) = child.lock() {
                            *held = fresh;
                        }
                        log::info("tunnel.reopened", fields! { "host" => forward.host.clone() });
                    }
                    Err(refusal) => log::warn(
                        "tunnel.reopen_failed",
                        fields! {
                            "host" => forward.host.clone(),
                            "detail" => refusal,
                            "impact" => "this daemon stays unreachable and its panes stay as \
                                         they were; the window is otherwise unaffected",
                        },
                    ),
                }
            }
        });
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.forward.local_socket);
        let _ = std::fs::remove_file(&self.forward.control_path);
    }
}

fn spawn(forward: &Forward) -> Result<Child, String> {
    Command::new("ssh")
        .args(master_arguments(forward))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // Inherited, so ssh's own account of a refused key or an unknown host reaches whoever
        // is reading this run's output. Nothing here could reword it better.
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            format!(
                "could not run ssh to reach {} ({error}), so that daemon's panes are absent \
                 from the window and nothing else is affected. Check that ssh is on PATH.",
                forward.host,
            )
        })
}

/// What the environment looks like on the far end.
///
/// Asked for rather than guessed, so that the rules deciding where a daemon's socket lives
/// stay in the one place that has them. A shell one-liner spelling those rules over there
/// would be a second copy of the thing most likely to drift.
///
/// A non-login shell, which is what ssh runs a command in, so this sees what sshd sets and
/// what the user's rc file exports - not a full login environment. `HOME` is always there,
/// which is what the default path needs; anything more exotic is what naming the socket in
/// the config file is for.
pub fn remote_environment(
    host: &str,
    options: &[String],
) -> Result<BTreeMap<String, String>, String> {
    let mut arguments: Vec<String> = vec!["-o".to_string(), "BatchMode=yes".to_string()];
    arguments.extend(options.iter().cloned());
    arguments.push(host.to_string());
    arguments.push("env".to_string());

    let output =
        Command::new("ssh").args(&arguments).stdin(Stdio::null()).output().map_err(|error| {
            format!(
                "could not run ssh to ask {host} about itself ({error}). Check that ssh is on \
                 PATH."
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "ssh to {host} would not run ({}), so there is no way to work out where its \
             daemon is listening. Either name the socket in the config file's `socket` key, or \
             fix the connection - ssh's own message: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    Ok(parse_environment(&String::from_utf8_lossy(&output.stdout)))
}

/// Reads `env` output into names and values.
///
/// Lines with no `=` are the continuations of a multi-line value, and are dropped rather than
/// guessed at: nothing this reads for is ever multi-line, and inventing a rule for a case that
/// does not arise is how a parser gets a bug nobody can reproduce.
fn parse_environment(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_environment_reads_names_and_values() {
        let read = parse_environment("HOME=/home/dev\nSHELL=/bin/sh\n");
        assert_eq!(read.get("HOME").map(String::as_str), Some("/home/dev"));
        assert_eq!(read.get("SHELL").map(String::as_str), Some("/bin/sh"));
    }

    #[test]
    fn a_value_holding_an_equals_keeps_all_of_it() {
        let read = parse_environment("OPTS=a=b=c\n");
        assert_eq!(read.get("OPTS").map(String::as_str), Some("a=b=c"));
    }

    #[test]
    fn a_continuation_line_is_dropped_rather_than_guessed_at() {
        let read = parse_environment("GREETING=hello\nworld\nHOME=/home/dev\n");
        assert_eq!(read.get("GREETING").map(String::as_str), Some("hello"));
        assert_eq!(read.get("HOME").map(String::as_str), Some("/home/dev"));
        assert_eq!(read.len(), 2);
    }
}
