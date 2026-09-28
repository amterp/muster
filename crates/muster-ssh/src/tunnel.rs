use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use muster_core::diagnostics::{clock, log, poison};
use muster_core::fields;
use muster_core::reconnect::{self, Attempts};

use crate::remote::{Remote, client_arguments, quoted};

/// What a tunnel says about itself, to whoever is holding it.
///
/// Two states and not three, because they are the two a person can act on. Every drop and
/// every retry in between goes to the run log, which is where a sequence belongs; what reaches
/// the window is whether this machine is worth waiting for.
#[derive(Debug, Clone)]
pub enum State {
    /// It has been down long enough to be worth telling somebody, and here is what to say.
    Unreachable { detail: String },

    /// It is up and has held long enough to count, so anything said about it can be taken
    /// back.
    Reachable,
}

/// How a tunnel reports itself. Runs on the supervising thread.
///
/// A callback rather than a problem raised from here, because this crate is transport and
/// knows nothing about windows, rosters or daemons - and the sentence it hands over is about a
/// host, which is the one thing it does know.
pub type Report = Arc<dyn Fn(State) + Send + Sync>;

/// What one master forwards, and how it is reached.
#[derive(Debug, Clone)]
pub struct Forward {
    /// An ssh destination, spelled however ssh accepts it: a host, `user@host`, or an alias
    /// out of the user's own config.
    pub host: String,
    /// Handed to ssh verbatim, after Muster's own options. Connection details belong in
    /// `~/.ssh/config`; this is the escape hatch for what a host alias cannot cover.
    pub options: Vec<String>,
    /// Where the master's control socket goes, so that a command on the far machine ([`Remote`])
    /// can run through this connection rather than opening one of its own.
    pub control_path: String,
    /// The path on this machine that will answer as though it were the daemon's own.
    pub local_socket: String,
    /// The daemon's socket, over there.
    pub remote_socket: String,
    /// A socket on this machine to answer on the far one as well, for as long as this master
    /// lives. Absent when nothing here is to be reachable from over there.
    pub reverse: Option<Reverse>,
}

/// A socket here, made to answer at a path over there: the other direction from [`Forward`]'s
/// own.
#[derive(Debug, Clone)]
pub struct Reverse {
    /// Where it answers on the far machine. Absolute, and in a directory that is made if it is
    /// missing.
    pub remote_path: String,
    /// The socket on this machine a connection over there reaches.
    pub local_path: String,
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
        // Whether the child Muster spawned is the connection at all. `ControlPersist` is a
        // common thing to have in a personal ssh config, and with it set `ssh -N -M` forks
        // into the background once it has authenticated - so the process Muster holds a
        // handle to has already exited while a different one carries the forward. A
        // supervisor watching that handle called a working connection down 31 times in
        // thirteen minutes, and a quit left eighteen authenticated connections behind
        // (kan a_2J1KZ9FbM, a_2J1KYPWhZ). Nothing below this line asks a pid about a master
        // any more, but the option stays pinned: a master in the foreground is one process
        // per daemon rather than two, and the escape hatch exists for connection details
        // rather than for how Muster supervises its own child.
        "-o".to_string(),
        "ControlPersist=no".to_string(),
    ];
    arguments.extend(forward.options.iter().cloned());
    arguments.push(forward.host.clone());
    arguments
}

/// What ssh is told to add a [`Reverse`] to a master that is already running.
///
/// Asked of the master through its control path rather than given to it at the start, because
/// the master runs with `ExitOnForwardFailure`. A far path it could not bind - a file left there,
/// an sshd that allows no socket forwarding - would otherwise take the whole connection down, and
/// every pane on that machine with it, over something the panes do not need to be drawn.
pub fn reverse_arguments(forward: &Forward, reverse: &Reverse) -> Vec<String> {
    vec![
        "-O".to_string(),
        "forward".to_string(),
        "-R".to_string(),
        format!("{}:{}", reverse.remote_path, reverse.local_path),
        "-S".to_string(),
        forward.control_path.clone(),
        forward.host.clone(),
    ]
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
    /// here would put the daemon's vocabulary in a crate that has none.
    pub fn open(forward: Forward, report: Report) -> Result<Tunnel, String> {
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
        // Paths left behind by a run that crashed, and ssh will not replace either: a stale
        // local socket binds nothing and refuses everything, and `-M -S` onto a path that
        // exists disables multiplexing with a warning instead of failing, which leaves every
        // bridge dialing a master that is gone (kan a_2IRdZK6Un). Safe because both names
        // carry this process's id.
        let _ = std::fs::remove_file(&forward.local_socket);
        let _ = std::fs::remove_file(&forward.control_path);

        let child = spawn(&forward)?;
        let tunnel = Tunnel {
            forward,
            child: Arc::new(Mutex::new(child)),
            stopping: Arc::new(AtomicBool::new(false)),
        };
        tunnel.wait_for_socket()?;
        forward_back(&tunnel.forward);
        tunnel.supervise(report);
        Ok(tunnel)
    }

    /// The path on this machine that answers as the daemon's own socket.
    pub fn local_socket_path(&self) -> &str {
        &self.forward.local_socket
    }

    /// The daemon's own socket, over there, as the far side spells it.
    ///
    /// Everything on this machine takes [`Tunnel::local_socket_path`] and never learns ssh was
    /// involved.
    pub fn remote_socket_path(&self) -> &str {
        &self.forward.remote_socket
    }

    /// The master's control socket, for a command that wants to ride this connection.
    pub fn control_path(&self) -> &str {
        &self.forward.control_path
    }

    /// The machine at the other end, as something that can be asked to do things.
    ///
    /// A value rather than a borrow, so that holding one does not hold the tunnel still. It
    /// stops working when the tunnel is dropped, which is the honest lifetime: the master is
    /// what makes it free.
    pub fn remote(&self) -> Remote {
        Remote::over(&self.forward.host, &self.forward.control_path)
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
            if let Ok(Some(status)) = poison::lock(&self.child, "ssh-child").try_wait() {
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

    /// Brings the connection back when it dies, and says when it cannot.
    ///
    /// The local path is the same across a restart, so nothing that holds it needs to learn
    /// that ssh went away - the adapter's own reconnect finds the socket answering again and
    /// resyncs. That is the payoff of forwarding a socket rather than running a command per
    /// request: recovery is a mechanism that already exists.
    ///
    /// **A reopen is a connection that answered, not a process that started.** `spawn` only
    /// reports whether ssh could be executed, so every real failure - a host that is not
    /// reachable, a key that needs a passphrase, a forward refused because something else
    /// holds it - comes back as `Ok` with a process about to die. Announcing that as a reopen
    /// is how 97 of them were logged in two minutes while nothing reconnected, which made the
    /// log worse than silent (kan a_2IRdZK6Un). So a reopen is announced when the forwarded
    /// socket exists and a command has come back over the master.
    ///
    /// **And the backoff escalates, because that one is what stopped it.** The old loop reset
    /// its attempt count whenever the child was alive at a poll, and an ssh that lives a
    /// quarter of a second before losing its forward is alive at a poll - so a laptop plugged
    /// back in retried at 1.25s forever. A run of failures now ends when the connection has
    /// *held*, which `muster_core::reconnect` decides and which nothing about a process
    /// existing can satisfy.
    ///
    /// **And "up" is asked of the control path rather than of a pid.** A pid is the master
    /// only while ssh stays in the foreground: with `ControlPersist` set, `ssh -N -M` forks
    /// after authenticating, and this loop spent thirteen minutes calling a connection down
    /// 250ms after confirming it - unlinking both paths out from under a master that was
    /// carrying every pane's traffic (kan a_2J1KZ9FbM). `master_arguments` pins the option
    /// away, and asking the path as well means no future ssh can put the bug back.
    fn supervise(&self, report: Report) {
        let forward = self.forward.clone();
        let child = Arc::clone(&self.child);
        let stopping = Arc::clone(&self.stopping);
        std::thread::spawn(move || {
            let mut attempts = Attempts::new();
            // Whether the last check went unanswered, which is what one more is allowed.
            let mut silent = false;
            while !stopping.load(Ordering::Relaxed) {
                if !sleep_unless_stopping(HEALTH_POLL, &stopping) {
                    return;
                }
                // Reaped rather than read. The answer is no longer the health check, but a
                // `Child` nobody waits on is a zombie and this loop is the only place holding
                // the handle.
                let _ = poison::lock(&child, "ssh-child").try_wait();

                let down = match control(&forward, "check") {
                    Ok(()) => {
                        silent = false;
                        // `ExitOnForwardFailure` means a master that is still running is one
                        // whose forward stood up, so a master that answers is the honest
                        // reading of "up" once a reopen has been confirmed once. What it
                        // cannot say is that the connection has *worked*, which is what ends a
                        // run of failures and is the only thing time can answer.
                        if attempts.holding(clock::monotonic_now()) {
                            log::info("tunnel.settled", fields! { "host" => forward.host.clone() });
                            report(State::Reachable);
                        }
                        continue;
                    }
                    // Asked again before anything is ended, because a check that times out is
                    // never a dropped network: a master whose connection dies exits once
                    // ServerAlive gives up, and the check after that fails at once. What is left
                    // is a master that is slow or wedged, or a checking ssh that was slow to
                    // start. Ending on the first cost a loaded Mac a master that was carrying
                    // seven panes, every one of which lost its bridge (kan a_2YQCqiInL); asking
                    // once more costs a wedged master one poll.
                    Err(Unanswered::Silent(detail)) if !silent => {
                        silent = true;
                        log::warn(
                            "tunnel.slow",
                            fields! {
                                "host" => forward.host.clone(),
                                "detail" => detail,
                                "impact" => "nothing yet: the master is asked again on the next \
                                             poll, and ended only if it stays silent",
                                "check" => "whether this machine is heavily loaded, or whether \
                                            the personal ssh config does slow work for every \
                                            ssh it runs, such as a `Match exec`",
                            },
                        );
                        continue;
                    }
                    Err(failure) => {
                        silent = false;
                        failure.into_detail()
                    }
                };

                let retry = attempts.failed();
                if retry.logged {
                    log::warn(
                        "tunnel.down",
                        fields! {
                            "host" => forward.host.clone(),
                            "attempt" => retry.attempt.to_string(),
                            "retry_in_ms" => (retry.after / 1_000_000).to_string(),
                            "detail" => down,
                            "impact" => "every pane on this daemon is rendering what it last \
                                         showed, and its agent states are a guess about the \
                                         present",
                            "check" => "whether the host is reachable - the connection is being \
                                        retried and recovers on its own once it is",
                        },
                    );
                }
                if retry.report {
                    report(State::Unreachable {
                        detail: reconnect::unreachable(&forward.host, retry.attempt),
                    });
                }

                if !sleep_unless_stopping(Duration::from_nanos(retry.after), &stopping) {
                    return;
                }

                if !reopen(&forward, &child, &stopping) {
                    return;
                }
            }
        });
    }
}

/// Ends the master that stopped answering and starts another, saying whether it is worth going
/// on: `false` when the tunnel was dropped while this was under way.
fn reopen(forward: &Forward, child: &Arc<Mutex<Child>>, stopping: &Arc<AtomicBool>) -> bool {
    // Ended rather than abandoned, and this is where the leak started: the loop
    // used to unlink both paths and spawn over the top, so a master that was
    // still connected went on holding an authenticated session nothing could
    // reach. Nineteen of them were counted against one control path, one per
    // reopen (kan a_2J1KYPWhZ). Doing it here rather than sweeping at `Drop` time
    // stops them accumulating instead of tidying up after.
    end_master(forward, child);
    match spawn(forward) {
        Ok(fresh) => {
            *poison::lock(child, "ssh-child") = fresh;
            if stopping.load(Ordering::Relaxed) {
                // Dropped while this reconnect was in flight. `Drop` asked the
                // control path to leave before this master bound it, so it is one
                // nothing else will ever end.
                end_master(forward, child);
                return false;
            }
            match confirm(forward, child, stopping) {
                Ok(()) => {
                    log::info(
                        "tunnel.reopened",
                        fields! {
                            "host" => forward.host.clone(),
                            "confirmed" => "the forwarded socket is bound and a \
                                            command came back over the master",
                        },
                    );
                    // At the path it had before, so whatever was told that path
                    // while the old master carried it reaches this one.
                    forward_back(forward);
                }
                Err(detail) => log::warn(
                    "tunnel.reopen_failed",
                    fields! {
                        "host" => forward.host.clone(),
                        "detail" => detail,
                        "impact" => "this daemon stays unreachable and its panes \
                                     stay as they were; the window is otherwise \
                                     unaffected",
                    },
                ),
            }
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
    true
}

/// The longest a sleeping supervisor waits before noticing the tunnel is being taken down.
///
/// Every wait in here is sliced by this, so dropping a tunnel does not sit out a thirty-second
/// backoff before the thread reacts - which on quit is a window that will not close.
const STOP_CHECK: Duration = Duration::from_millis(250);

/// How often the supervisor asks whether the master is still there.
///
/// Slower than the slice above, because the question is a subprocess now rather than a syscall
/// on a handle. Nothing recovers sooner for asking four times a second: ssh's own
/// `ServerAlive` settings take about forty-five seconds to notice a black-holed connection,
/// and the first retry after a real drop waits 1.25s regardless.
const HEALTH_POLL: Duration = Duration::from_secs(1);

/// How long a control request gets before the master is treated as not answering.
///
/// Bounded because a wedged master answers nothing, and both callers run somewhere that cannot
/// afford to block: the supervising thread, and `Drop` on the way to closing a window.
const CONTROL_WITHIN: Duration = Duration::from_secs(2);

/// How long a reopened master gets to bind its socket and answer, before the attempt counts as
/// having failed.
///
/// Shorter than the timeout a first connection gets, because this one is inside a retry loop
/// that will come round again: waiting twenty seconds here would make a machine that is simply
/// away take twenty seconds per attempt to say so.
const CONFIRM_WITHIN: Duration = Duration::from_secs(5);

/// Sleeps, unless the tunnel is being taken down. Answers whether it is worth going on.
fn sleep_unless_stopping(wait: Duration, stopping: &Arc<AtomicBool>) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if stopping.load(Ordering::Relaxed) {
            return false;
        }
        // Saturating, because the deadline can pass between the check and the subtraction and
        // `Instant - Instant` panics on a negative result rather than answering zero. Zero
        // left is also the exit, so the two are one test.
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(STOP_CHECK.min(left));
    }
}

/// Asks the master on this control path to do something, and does not wait forever for it.
///
/// Addressed to the path rather than to a pid, which is the difference the two cards behind
/// this turn on: `-O` requests reach whichever process currently owns the path, and a pid is
/// the master only while ssh stays in the foreground. `check` asks whether a master is there;
/// `exit` asks it to leave, and it removes the control path on its way out.
///
/// A local question either way - a control request rides the multiplexing socket and never
/// touches the network - so this says a master is there, not that its connection is carrying
/// anything. Whether it *works* is [`confirm`], which is worth a round trip once per reopen
/// and not once a second.
///
/// ssh's own words are folded into the error rather than inherited, unlike [`spawn`]: a host
/// that is away would otherwise print "Control socket connect: No such file or directory" to
/// this process's stderr every second for as long as the outage lasts.
fn control(forward: &Forward, request: &str) -> Result<(), Unanswered> {
    bounded(
        forward,
        &format!("-O {request}"),
        ["-O", request, "-S", &forward.control_path, &forward.host],
    )
}

/// Why a request through the control path came to nothing.
///
/// Two kinds, because they mean different things to the supervisor: a request that failed at
/// once found nothing answering on the path, and one that timed out found something holding it
/// that did not answer in time.
#[derive(Debug)]
enum Unanswered {
    /// Nothing answered within [`CONTROL_WITHIN`], and the request was killed.
    Silent(String),
    /// It could not be run, or it finished and said no.
    Failed(String),
}

impl Unanswered {
    fn into_detail(self) -> String {
        match self {
            Unanswered::Silent(detail) | Unanswered::Failed(detail) => detail,
        }
    }
}

/// Runs one ssh through the master's control path, and gives up on it after [`CONTROL_WITHIN`].
fn bounded<I, S>(forward: &Forward, what: &str, arguments: I) -> Result<(), Unanswered>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut asked = Command::new("ssh")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            Unanswered::Failed(format!("could not run ssh to ask for `{what}` ({error})"))
        })?;

    let deadline = Instant::now() + CONTROL_WITHIN;
    loop {
        match asked.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(Unanswered::Failed(format!(
                    "the master refused `{what}` ({status}): {}",
                    stderr_of(asked)
                )));
            }
            Err(error) => {
                return Err(Unanswered::Failed(format!("ssh would not finish `{what}` ({error})")));
            }
            Ok(None) => {}
        }
        if Instant::now() >= deadline {
            let _ = asked.kill();
            let _ = asked.wait();
            return Err(Unanswered::Silent(format!(
                "the master on {} did not answer `{what}` within {}s",
                forward.control_path,
                CONTROL_WITHIN.as_secs(),
            )));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Makes this master's [`Reverse`] answer over there, if it has one.
///
/// The far path is cleared first. sshd binds a socket path only where nothing is, unless its own
/// `StreamLocalBindUnlink` says otherwise, and a master that went away without saying so leaves
/// its file behind. The path is this master's alone, so nothing else is using it.
///
/// A failure is a warning and nothing more: the panes on that machine are still drawn and typed
/// into, and only a program there asking for this socket finds nothing.
fn forward_back(forward: &Forward) {
    let Some(reverse) = &forward.reverse else { return };
    let outcome = if reverse.remote_path.len() > SUN_PATH_LIMIT {
        Err(format!(
            "{} is {} bytes of path, and a unix socket has about {SUN_PATH_LIMIT} to spend",
            reverse.remote_path,
            reverse.remote_path.len()
        ))
    } else {
        let directory = reverse.remote_path.rsplit_once('/').map_or(".", |(parent, _)| parent);
        Remote::over(&forward.host, &forward.control_path)
            .shell(&format!(
                "mkdir -p {} && rm -f {}",
                quoted(directory),
                quoted(&reverse.remote_path)
            ))
            .and_then(|_| {
                bounded(forward, "-O forward -R", reverse_arguments(forward, reverse))
                    .map_err(Unanswered::into_detail)
            })
    };
    match outcome {
        Ok(()) => log::info(
            "tunnel.reverse",
            fields! {
                "host" => forward.host.clone(),
                "remote" => reverse.remote_path.clone(),
                "local" => reverse.local_path.clone(),
            },
        ),
        Err(detail) => log::warn(
            "tunnel.reverse_failed",
            fields! {
                "host" => forward.host.clone(),
                "remote" => reverse.remote_path.clone(),
                "detail" => detail,
                "impact" => "a program on that machine cannot reach this side through that path; \
                             its panes are drawn and typed into as before",
                "check" => "whether that machine's sshd allows socket forwarding \
                            (AllowStreamLocalForwarding) and whether the path can be made",
            },
        ),
    }
}

/// Takes this master's [`Reverse`] off the far machine, so the file left there does not look
/// like something that might answer.
///
/// Bounded, because it runs on the way to closing a window, and a link that has gone quiet would
/// otherwise hold that up for as long as ssh takes to notice.
fn take_back(forward: &Forward) {
    let Some(reverse) = &forward.reverse else { return };
    let mut arguments = client_arguments(&forward.host, &forward.control_path);
    arguments.extend(["rm".to_string(), "-f".to_string(), quoted(&reverse.remote_path)]);
    let removed = bounded(forward, "rm -f", arguments);
    if let Err(detail) = removed.map_err(Unanswered::into_detail) {
        log::debug(
            "tunnel.reverse_left",
            fields! {
                "host" => forward.host.clone(),
                "remote" => reverse.remote_path.clone(),
                "detail" => detail,
            },
        );
    }
}

/// What a finished ssh had to say for itself, in one line.
fn stderr_of(mut asked: Child) -> String {
    let mut text = String::new();
    if let Some(mut stderr) = asked.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Ends the master behind this tunnel, whichever process that turns out to be, and takes both
/// its paths with it.
///
/// Killing the child ends the connection only while the child is the master, and one quit left
/// eighteen authenticated connections running on the far machine because it was not
/// (kan a_2J1KYPWhZ). So the master is asked to leave through its control path first; the kill
/// stays as the fallback for one that never answered, and is also what reaps the handle.
fn end_master(forward: &Forward, child: &Arc<Mutex<Child>>) {
    let asked = control(forward, "exit").map_err(Unanswered::into_detail);
    {
        let mut child = poison::lock(child, "ssh-child");
        let _ = child.kill();
        let _ = child.wait();
    }
    log::debug(
        "tunnel.master_ended",
        fields! {
            "host" => forward.host.clone(),
            "control_path" => forward.control_path.clone(),
            "asked" => match &asked {
                Ok(()) => "the master left when the control path asked it to".to_string(),
                Err(refusal) => format!("fell back to killing the child: {refusal}"),
            },
        },
    );
    let _ = std::fs::remove_file(&forward.local_socket);
    let _ = std::fs::remove_file(&forward.control_path);
}

/// Whether a master that was just started is actually carrying anything.
///
/// Two questions, because the first alone is the check `daemon::answers` already argues
/// against: a socket path is a file, and one being there says nothing about what is behind it.
/// The second rides the master's own control path, so it proves the multiplexing every pane's
/// bridge depends on, and it needs no vocabulary from any daemon.
fn confirm(
    forward: &Forward,
    child: &Arc<Mutex<Child>>,
    stopping: &Arc<AtomicBool>,
) -> Result<(), String> {
    let deadline = Instant::now() + CONFIRM_WITHIN;
    while !Path::new(&forward.local_socket).exists() {
        if stopping.load(Ordering::Relaxed) {
            return Err("the tunnel was taken down while it was being checked".to_string());
        }
        if let Ok(Some(status)) = poison::lock(child, "ssh-child").try_wait() {
            return Err(format!(
                "ssh ended before it forwarded anything ({status}); its own message is on this \
                 run's error output"
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "no socket appeared at {} within {}s",
                forward.local_socket,
                CONFIRM_WITHIN.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Remote::over(&forward.host, &forward.control_path)
        .run(&["true"])
        .map(|_| ())
        .map_err(|error| format!("the master would not carry a command ({error})"))
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        take_back(&self.forward);
        end_master(&self.forward, &self.child);
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
///
/// Bounded by a connect timeout, placed after the config's own options so that one named there
/// wins: the first reach of a machine that is not answering would otherwise wait out the
/// operating system's TCP timeout, a minute or more, for each attempt.
pub fn remote_environment(
    host: &str,
    options: &[String],
) -> Result<BTreeMap<String, String>, String> {
    let mut arguments: Vec<String> = vec!["-o".to_string(), "BatchMode=yes".to_string()];
    arguments.extend(options.iter().cloned());
    arguments.extend(["-o".to_string(), "ConnectTimeout=15".to_string()]);
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
    fn a_reverse_forward_is_asked_of_the_master_by_its_control_path() {
        let forward = Forward {
            host: "dev@devenv".to_string(),
            options: vec!["-p".to_string(), "2222".to_string()],
            control_path: "/tmp/m.ctl".to_string(),
            local_socket: "/tmp/m.sock".to_string(),
            remote_socket: "/home/dev/.muster/daemon/d.sock".to_string(),
            reverse: None,
        };
        let reverse = Reverse {
            remote_path: "/home/dev/.muster/daemon/window-w1.sock".to_string(),
            local_path: "/Users/me/.muster/state/command-1.sock".to_string(),
        };
        assert_eq!(
            reverse_arguments(&forward, &reverse),
            [
                "-O",
                "forward",
                "-R",
                "/home/dev/.muster/daemon/window-w1.sock:/Users/me/.muster/state/command-1.sock",
                "-S",
                "/tmp/m.ctl",
                "dev@devenv",
            ]
        );
    }

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
