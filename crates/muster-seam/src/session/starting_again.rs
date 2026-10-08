//! Starting a daemon Muster manages again, once it has gone.
//!
//! A devenv rebooted and took its daemon with it, and the window went on retrying a socket
//! nothing would ever answer again: its panes stayed stale until Muster was relaunched, which is
//! the only thing that started a daemon (kan a_2cJ6cgiLw). A window attached to a machine is
//! asking to show it, so the start a launch would make is made here too, the moment the daemon
//! stops answering - and the daemon it starts brings its panes back from its saved state, with
//! every agent whose harness reported its session resumed in it.
//!
//! Only a daemon Muster would start at launch: its own on this machine, and the one it installs
//! on an ssh host. A socket somebody named in the config is theirs, and is left alone here as it
//! is at launch.
//!
//! What makes it safe to try is that starting is never blind. Each attempt asks whether anything
//! answers first: a daemon that answers is adopted and the follower takes it from there, one
//! that holds its socket and does not answer is wedged rather than gone and is never started
//! over, and two starts at once leave one daemon, since a daemon holds a lock beside its socket
//! and a second exits.
//!
//! On an ssh host the question is asked through the forward, which cannot tell a daemon that
//! has gone from a connection that has: a tunnel that flapped, or a laptop waking, reads as
//! nothing answering. So nothing is started there while the connection itself is down.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use muster_core::composition::DaemonId;
use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::Health;
use muster_core::reconnect;
use muster_daemon_client::{install as remote_install, launch, remote};
use muster_daemon_proto::Welcome;

use super::{SESSION, Unattached, start_here};

/// How to start a daemon again.
#[derive(Debug, Clone)]
pub(crate) enum StartAgain {
    /// Muster's own daemon on this machine, found where a launch finds it.
    Here,
    /// The daemon Muster installs on an ssh host, through the forward it already holds.
    There(Box<There>),
}

/// What starting the daemon on an ssh host takes, as attaching it worked it out.
#[derive(Debug, Clone)]
pub(crate) struct There {
    pub host: String,
    pub remote: muster_ssh::Remote,
    pub installed: remote::Installed,
    pub carried: remote_install::Carried,
    /// The daemon's whole environment over there (`environment::for_far_daemon`).
    pub environment: BTreeMap<String, String>,
    /// The forward's end on this machine.
    pub local: PathBuf,
}

impl There {
    pub(crate) fn ensure_running(&self) -> Result<(launch::Reached, Welcome), String> {
        remote::ensure_running(
            &self.remote,
            &self.installed,
            &self.carried,
            &self.local,
            &self.environment,
        )
    }
}

/// Why an attempt started nothing.
enum NotStarted {
    /// Whether the daemon has gone cannot be told yet, so nothing was tried.
    Unknowable(String),
    /// It was tried, and this is why it failed.
    Failed(String),
}

impl StartAgain {
    /// Where it runs, for a log line.
    fn host(&self) -> &str {
        match self {
            StartAgain::Here => "this machine",
            StartAgain::There(there) => &there.host,
        }
    }

    fn ensure_running(&self) -> Result<launch::Reached, NotStarted> {
        match self {
            StartAgain::Here => start_here(
                "this machine's panes stay unreachable in the window until its daemon answers",
            )
            .map(|here| here.reached)
            .map_err(|unattached| {
                NotStarted::Failed(match unattached {
                    Unattached::Failed(why) | Unattached::Lasting(why) => why,
                    Unattached::Abandoned => "the window was reset".to_string(),
                })
            }),
            // Asked first, because with the master down the forward answers nothing whether or
            // not the daemon is there, and the start would install over a live daemon's files.
            StartAgain::There(there) if !there.remote.master_answers() => {
                Err(NotStarted::Unknowable(format!(
                    "the ssh connection to {} is down, so a daemon that stopped answering cannot                      be told from a connection that did",
                    there.host
                )))
            }
            StartAgain::There(there) => {
                there.ensure_running().map(|(reached, _)| reached).map_err(NotStarted::Failed)
            }
        }
    }
}

/// The daemons a thread is starting again, so that a daemon that goes stale on every reconnect
/// it tries is started by one thread at a time - each with the claim of the thread starting it.
static STARTING: Mutex<BTreeMap<DaemonId, u64>> = Mutex::new(BTreeMap::new());

/// Where each claim's number comes from.
static CLAIMS: AtomicU64 = AtomicU64::new(0);

/// One thread's claim on starting a daemon again, given back when it is dropped - by a thread
/// that finished, panicked or never started alike.
///
/// Numbered, because [`forget`] empties the set while a thread from before may still be asleep
/// in its backoff: when that one wakes and lets go, it must not take back a claim a thread
/// started since has made on the same daemon.
struct Claim {
    daemon: DaemonId,
    number: u64,
}

impl Claim {
    /// Claims `daemon`, unless a thread is starting it already.
    fn on(daemon: &DaemonId) -> Option<Claim> {
        let number = CLAIMS.fetch_add(1, Ordering::Relaxed);
        let mut starting = poison::lock(&STARTING, "starting-again");
        if starting.contains_key(daemon) {
            return None;
        }
        starting.insert(daemon.clone(), number);
        Some(Claim { daemon: daemon.clone(), number })
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut starting = poison::lock(&STARTING, "starting-again");
        if starting.get(&self.daemon) == Some(&self.number) {
            starting.remove(&self.daemon);
        }
    }
}

/// Forgets which daemons are being started again, for a session starting over. A thread still
/// asleep finds the daemon no longer followed when it wakes, and stops.
pub(super) fn forget() {
    poison::lock(&STARTING, "starting-again").clear();
}

/// `daemon` has stopped answering: starts it again, on a thread of its own, if Muster manages
/// it and nothing is starting it already.
pub(super) fn gone(daemon: &DaemonId) {
    let (start, mirror) = {
        let session = poison::lock(&SESSION, "session");
        if session.quitting {
            return;
        }
        let Some(backend) = session.backends.get(daemon) else { return };
        let Some(start) = backend.start_again.clone() else { return };
        (start, Arc::clone(&backend.mirror))
    };
    let Some(claim) = Claim::on(daemon) else { return };
    let spawned =
        std::thread::Builder::new().name(format!("muster-start-again-{daemon}")).spawn(move || {
            start_again(&claim.daemon, &start, &mirror);
        });
    // A thread that could not be made dropped its claim with the closure that held it.
    if let Err(error) = spawned {
        log::warn(
            "daemon.start_again.unstarted",
            fields! {
                "daemon" => daemon.to_string(),
                "error" => error.to_string(),
                "impact" => "nothing starts this daemon again, so its panes stay unreachable \
                             until something else does or Muster is relaunched",
                "check" => "whether this process has run out of threads",
            },
        );
    }
}

/// Tries to start `daemon` on the reconnect backoff for as long as it is gone and this session
/// still follows it.
fn start_again(daemon: &DaemonId, start: &StartAgain, mirror: &Arc<Mutex<Mirror>>) {
    let mut attempts = reconnect::Attempts::new();
    while still_gone(daemon, mirror) {
        match start.ensure_running() {
            Ok(reached) => {
                answered(daemon, start.host(), mirror, reached);
                return;
            }
            Err(NotStarted::Unknowable(why)) => {
                let retry = attempts.failed();
                if retry.logged {
                    log::info(
                        "daemon.start_again.waiting",
                        fields! {
                            "daemon" => daemon.to_string(),
                            "host" => start.host(),
                            "detail" => why,
                            "impact" => "nothing is started there yet; its panes read \
                                         unreachable until the connection is back, and then \
                                         either its daemon answers or it is started again",
                        },
                    );
                }
                std::thread::sleep(std::time::Duration::from_nanos(retry.after));
            }
            Err(NotStarted::Failed(why)) => {
                let retry = attempts.failed();
                if retry.logged {
                    log::warn(
                        "daemon.start_again.failed",
                        fields! {
                            "daemon" => daemon.to_string(),
                            "host" => start.host(),
                            "attempt" => attempts.failures(),
                            "detail" => why,
                            "impact" => "this daemon's panes stay unreachable in the window; \
                                         Muster tries again on the reconnect backoff",
                            "check" => "the detail says which step failed. For a devenv, \
                                        whether it is reachable over ssh at all; a daemon \
                                        that answers nothing while holding its socket is \
                                        stuck rather than gone, and is never started over - \
                                        its log is beside its socket",
                        },
                    );
                }
                std::thread::sleep(std::time::Duration::from_nanos(retry.after));
            }
        }
    }
}

/// Whether this session still follows the daemon with this mirror, and it still does not
/// answer. A reset, a detach or a quit ends the attempts; so does the daemon answering, whoever
/// started it.
fn still_gone(daemon: &DaemonId, mirror: &Arc<Mutex<Mirror>>) -> bool {
    let session = poison::lock(&SESSION, "session");
    !session.quitting
        && session.backends.get(daemon).is_some_and(|backend| Arc::ptr_eq(&backend.mirror, mirror))
        && poison::lock(mirror, "mirror").health() != Health::Connected
}

/// A daemon answers again, started by this attempt or by somebody else - `muster msg` starts
/// one too. Either way the follower is told to dial it now: it may be part-way through a
/// reconnect backoff of half a minute, and the panes would read unreachable for all of it.
fn answered(daemon: &DaemonId, host: &str, mirror: &Arc<Mutex<Mirror>>, reached: launch::Reached) {
    let started = reached == launch::Reached::Started;
    if started {
        log::info(
            "daemon.started_again",
            fields! {
                "daemon" => daemon.to_string(),
                "host" => host,
                "impact" => "every process in its panes is new: they come back from its saved \
                             state, and an agent comes back in its session only where its \
                             harness reported one",
            },
        );
    } else {
        log::info(
            "daemon.start_again.adopted",
            fields! { "daemon" => daemon.to_string(), "host" => host },
        );
    }
    let mut session = poison::lock(&SESSION, "session");
    let Some(backend) =
        session.backends.get_mut(daemon).filter(|backend| Arc::ptr_eq(&backend.mirror, mirror))
    else {
        return;
    };
    // Muster's now, whoever started the one before: `muster window` says so, which is what
    // tells somebody deciding whether to end it that this window made it.
    if started {
        backend.started = true;
    }
    backend.follower.retry_now();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_from_before_a_reset_lets_go_of_nothing_claimed_since() {
        let daemon = DaemonId::new("claimed-across-a-reset");
        let before = Claim::on(&daemon).expect("nothing is starting it");
        assert!(Claim::on(&daemon).is_none(), "two threads may start one daemon");

        forget();
        let since = Claim::on(&daemon).expect("a reset forgets every claim");
        drop(before);
        assert!(Claim::on(&daemon).is_none(), "the old claim let go of the new one");

        drop(since);
        assert!(Claim::on(&daemon).is_some(), "a dropped claim is given back");
    }
}
