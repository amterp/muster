//! Attaching the daemons a window follows: the ones a config file names, each on a thread of its
//! own, and the one on this machine when it names none.
//!
//! Separate from the live session because it runs beside it rather than inside it. An attach can
//! take as long as an ssh connection or a daemon's start does, so it never holds the session's
//! lock while it waits; it takes a ticket stamped with the session's generation, and a reset
//! that replaces the session makes every ticket from before it worthless, so an attach that
//! finishes late throws its daemon away rather than landing it in a session that never asked for
//! it. The window opens after at most [`GRACE`], without the daemons still on their way, and
//! those are reconciled into it as they answer.

use super::{
    BTreeMap, BTreeSet, Config, Daemon, DaemonId, Endpoint, Event, Health, LOCAL, Mutex, Path,
    PathBuf, SESSION, Saved, Session, Severity, TabId, Tunnel, WindowId, clear_problem,
    daemon_binary, event, ffi, fields, following_anything, hand_over_later, health, is_following,
    log, poison, publish, raise_problem, reach, reconnect, settle_what_every_window_shows, startup,
};

/// Starts following every daemon a config file named, each on a thread of its own, and waits
/// at most [`GRACE`] for them.
///
/// No regions yet. Which tab a region shows depends on where the pane in `argv` turned out to
/// live, and that is not known until [`attach`] has asked - so opening one here would mean
/// opening a second one a moment later and closing the first.
///
/// This runs before the shell has made a window, so whatever it waits for, the person waits for
/// with nothing on screen. A running daemon on this machine answers in milliseconds and a quick
/// devenv inside the grace, so an ordinary launch still opens with every pane in it; a slower
/// one arrives in the open window, which already takes a daemon whose state comes late. The
/// threads run side by side, so two slow devenvs cost one wait rather than two.
///
/// A daemon that will not attach is tried again rather than given up on (`keep_attaching`). One
/// unreachable devenv should cost its own panes and nothing else, and a window that refused to
/// open because a container was down would be worse than no window at all.
pub(crate) fn follow_configured(config: &Config) {
    poison::lock(&APPLIED_DAEMONS, "applied-daemons").clone_from(&config.daemons);
    follow_in_background(&config.daemons);
}

/// The `[[daemon]]` blocks this process attached, as they were written when it did.
///
/// Compared against a config saved later, rather than the config read last, because a block
/// taken out and left waiting for a relaunch is still attached, and the next save should still
/// say so.
pub(super) static APPLIED_DAEMONS: Mutex<Vec<Daemon>> = Mutex::new(Vec::new());

/// Attaches the daemons a saved config adds, and answers the attached ones it takes out or
/// changes, which are left as they are.
///
/// Adding is safe to do live: a machine's panes arrive in the window as its attach finishes,
/// and nothing that was there moves. Taking one out is not. Its panes keep running on the
/// daemon whatever the file says, and detaching it would take their tabs out of every window,
/// which reads as agents gone - so it waits for a relaunch, and the caller says so. A block
/// whose id stays and whose endpoint changes is both at once, and waits whole.
///
/// A block whose daemon never attached - still being tried, or given up on - has no panes to
/// keep, so a changed endpoint is taken at once: its attach starts again from the new one, and
/// the one still retrying the old endpoint stops. Fixing a mistyped host is the usual reason.
pub(crate) fn follow_changed(config: &Config) -> Vec<Daemon> {
    // Asked first: the session's lock is taken before the attaches' everywhere else.
    let following: BTreeSet<DaemonId> = config
        .daemons
        .iter()
        .filter(|daemon| is_following(&daemon.id))
        .map(|daemon| daemon.id.clone())
        .collect();
    let added: Vec<Daemon> = {
        let mut attaches = poison::lock(&ATTACHES, "attaches");
        let mut applied = poison::lock(&APPLIED_DAEMONS, "applied-daemons");
        let mut added: Vec<Daemon> = config
            .daemons
            .iter()
            .filter(|daemon| applied.iter().all(|held| held.id != daemon.id))
            .filter(|daemon| {
                !following.contains(&daemon.id) && !attaches.under_way.contains(&daemon.id)
            })
            .cloned()
            .collect();
        applied.extend(added.iter().cloned());
        for daemon in &config.daemons {
            let Some(held) = applied.iter_mut().find(|held| held.id == daemon.id) else { continue };
            let unattached =
                attaches.under_way.contains(&daemon.id) || !following.contains(&daemon.id);
            if held != daemon && unattached {
                daemon.clone_into(held);
                // The attach under way, if any, stops counting now, before its backend is let go
                // below: one still current could make another in between.
                attaches.under_way.remove(&daemon.id);
                attaches.issue(&daemon.id);
                added.push(daemon.clone());
            }
        }
        added
    };
    // An attach waiting for the old endpoint's state has a backend for it, which the new attach
    // would otherwise take for its own.
    for daemon in added.iter().filter(|daemon| following.contains(&daemon.id)) {
        let removed = poison::lock(&SESSION, "session").let_go_of(&daemon.id);
        drop(removed);
    }
    if !added.is_empty() {
        log::info(
            "config.reload.daemons.attaching",
            fields! {
                "daemons" => added.iter().map(described).collect::<Vec<_>>().join(", "),
            },
        );
        follow_in_background(&added);
    }
    poison::lock(&APPLIED_DAEMONS, "applied-daemons")
        .iter()
        .filter(|held| !config.daemons.contains(held))
        .cloned()
        .collect()
}

/// Attaches each daemon on a thread of its own, retried until it answers, and waits at most
/// [`GRACE`] for them, as [`follow_configured`] says.
pub(super) fn follow_in_background(daemons: &[Daemon]) {
    let (generation, tickets) = {
        let mut attaches = poison::lock(&ATTACHES, "attaches");
        attaches.under_way.extend(daemons.iter().map(|daemon| daemon.id.clone()));
        let tickets: Vec<Ticket> =
            daemons.iter().map(|daemon| attaches.issue(&daemon.id)).collect();
        (attaches.generation, tickets)
    };
    for (daemon, ticket) in daemons.iter().zip(tickets) {
        let attaching = daemon.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("muster-attach-{}", daemon.id))
            .spawn(move || keep_attaching(&attaching, ticket));
        if let Err(error) = spawned {
            log::error(
                "daemon.unavailable",
                fields! {
                    "daemon" => daemon.id.to_string(),
                    "detail" => format!("no thread could be started to attach it ({error})"),
                    "impact" => "this daemon's panes are absent from the window until it is \
                                 relaunched; every other daemon in the config is unaffected",
                    "check" => "whether this process has run out of threads",
                },
            );
            attach_ended(&daemon.id, ticket);
        }
    }
    let grace = poison::lock(&SET_GRACE, "grace").unwrap_or(GRACE);
    let attaches = poison::lock(&ATTACHES, "attaches");
    let _waited = ATTACH_ENDED
        .wait_timeout_while(attaches, grace, |attaches| {
            attaches.generation == generation && !attaches.under_way.is_empty()
        })
        .unwrap_or_else(std::sync::PoisonError::into_inner);
}

/// How long starting waits for the daemons a config names before the window opens without the
/// ones still on their way. Past it, a window that has not appeared reads as a Muster that did
/// not start.
pub(super) const GRACE: std::time::Duration = std::time::Duration::from_secs(1);

/// [`GRACE`] as a test set it, until the next reset: a test whose subject is which daemon the
/// window starts on, rather than how long starting waits, sets it past any daemon it delays.
pub(super) static SET_GRACE: Mutex<Option<std::time::Duration>> = Mutex::new(None);

pub(crate) fn set_startup_grace(grace: std::time::Duration) {
    *poison::lock(&SET_GRACE, "grace") = Some(grace);
}

/// The configured daemons still being attached, and which launch they belong to.
///
/// Outside the session, because `reset` replaces the session wholesale and an attach from the
/// session it replaced must be told so: its generation no longer matches, and it throws its
/// daemon away rather than landing it in a session that never asked for it.
#[derive(Debug)]
pub(super) struct Attaches {
    pub(super) generation: u64,
    pub(super) under_way: BTreeSet<DaemonId>,
    /// The latest attach of each daemon. A reload that changes a block whose daemon never
    /// attached starts another, and the one before stops at its next look.
    latest: BTreeMap<DaemonId, u64>,
    issued: u64,
    /// Those under way whose first attempt has failed: still retried, and no longer waited for
    /// by anything that would rather have them first.
    pub(super) failed_once: BTreeSet<DaemonId>,
}

impl Attaches {
    /// Starts an attach of `daemon`, on its first attempt: whatever an attach it replaces had
    /// failed is that one's.
    fn issue(&mut self, daemon: &DaemonId) -> Ticket {
        self.issued += 1;
        self.latest.insert(daemon.clone(), self.issued);
        self.failed_once.remove(daemon);
        Ticket { generation: self.generation, attach: self.issued }
    }

    fn current(&self, daemon: &DaemonId, ticket: Ticket) -> bool {
        self.generation == ticket.generation && self.latest.get(daemon) == Some(&ticket.attach)
    }
}

/// Which attach of a daemon a thread is running: of which launch, and which of that daemon's.
#[derive(Debug, Clone, Copy)]
pub(super) struct Ticket {
    generation: u64,
    attach: u64,
}

pub(super) static ATTACHES: Mutex<Attaches> = Mutex::new(Attaches {
    generation: 0,
    under_way: BTreeSet::new(),
    latest: BTreeMap::new(),
    issued: 0,
    failed_once: BTreeSet::new(),
});

/// Told whenever an attach ends, for a caller waiting on the ones under way.
pub(super) static ATTACH_ENDED: std::sync::Condvar = std::sync::Condvar::new();

/// Whether an attach still belongs to the session there is now, and is its daemon's latest.
pub(super) fn attach_current(daemon: &DaemonId, ticket: Ticket) -> bool {
    poison::lock(&ATTACHES, "attaches").current(daemon, ticket)
}

pub(super) fn attach_ended(daemon: &DaemonId, ticket: Ticket) {
    let mut attaches = poison::lock(&ATTACHES, "attaches");
    if attaches.current(daemon, ticket) {
        attaches.under_way.remove(daemon);
    }
    ATTACH_ENDED.notify_all();
}

/// Waits up to `patience` for every configured daemon still attaching to be attached.
pub(super) fn wait_for_attaches(patience: std::time::Duration) {
    let attaches = poison::lock(&ATTACHES, "attaches");
    let _waited = ATTACH_ENDED
        .wait_timeout_while(attaches, patience, |attaches| !attaches.under_way.is_empty())
        .unwrap_or_else(std::sync::PoisonError::into_inner);
}

/// Whether a configured daemon is still being attached.
pub(super) fn attaching_anything() -> bool {
    !poison::lock(&ATTACHES, "attaches").under_way.is_empty()
}

/// Holds a link from each daemon on this machine to each the window reaches over ssh, as each
/// attaches: either end of a pair may be the one to arrive last. The far end is the local end
/// of its forward, which stays the same path when the tunnel reopens, and the daemon here dials
/// it again itself.
pub(super) fn link_daemons() {
    let mut session = poison::lock(&SESSION, "session");
    let ends: Vec<(DaemonId, bool, String)> = session
        .backends
        .iter()
        .map(|(id, backend)| (id.clone(), backend.tunnel.is_some(), backend.socket_path.clone()))
        .collect();
    for (near, remote, here) in &ends {
        for (far, far_remote, there) in &ends {
            let pair = (near.clone(), far.clone());
            if *remote || !*far_remote || session.peering.contains_key(&pair) {
                continue;
            }
            let held = crate::peering::Held::start(
                PathBuf::from(here),
                far.as_str().to_string(),
                PathBuf::from(there),
            );
            session.peering.insert(pair, held);
        }
    }
}

/// Attaches one configured daemon, trying again on the reconnect backoff for as long as the
/// session lasts.
///
/// A daemon that could not be reached at launch used to be dropped for the life of the process,
/// so a devenv whose VPN came up a minute after Muster did needed a relaunch - which costs the
/// panes on every other machine. Each failed attempt is a `daemon.unavailable` record, and the
/// first raises a problem that the attach clears. The first rather than the fifth, as for a
/// dropped connection: each attempt has already waited out its own patience, ten seconds for a
/// daemon's state and more for an ssh host, so one failure is already a machine missing from the
/// window for longer than anybody would wait without being told why.
pub(super) fn keep_attaching(daemon: &Daemon, ticket: Ticket) {
    let key = reconnect::key(daemon.id.as_str());
    let mut attempts = reconnect::Attempts::new();
    loop {
        connecting(&daemon.id);
        let attached = attach_daemon_in(daemon, ticket);
        // A corrected block may have started another attach meanwhile, and what this one found
        // out about the old endpoint is no longer anybody's news: no problem, no health, and
        // nothing that would make the new attach look as if it had failed.
        if !attach_current(&daemon.id, ticket) {
            return;
        }
        match attached {
            Ok(()) => {
                health(&daemon.id, Health::Connected, "");
                link_daemons();
                // Whatever the attempts before said, this one's or an attach it replaced.
                clear_problem(&key, "attached");
                attach_ended(&daemon.id, ticket);
                restore_late(&daemon.id);
                return;
            }
            Err(Unattached::Abandoned) => return,
            Err(Unattached::Lasting(refusal)) => {
                log::warn(
                    "daemon.unavailable.lasting",
                    fields! {
                        "daemon" => daemon.id.to_string(),
                        "detail" => &refusal,
                        "impact" => "this daemon's panes are absent from the window, and Muster \
                                     has stopped trying to attach it, because another attempt \
                                     would fail the same way",
                        "check" => "the problem's sentence names what to change; relaunch \
                                    Muster once it is changed",
                    },
                );
                health(&daemon.id, Health::Disconnected, &refusal);
                raise_problem(&key, Severity::Error, &stopped_attaching(daemon, &refusal));
                attach_ended(&daemon.id, ticket);
                return;
            }
            Err(Unattached::Failed(refusal)) => {
                let retry = attempts.failed();
                if retry.logged {
                    log::warn(
                        "daemon.unavailable",
                        fields! {
                            "daemon" => daemon.id.to_string(),
                            "detail" => &refusal,
                            "attempt" => attempts.failures(),
                            "impact" => "this daemon's panes are absent from the window until an \
                                         attempt succeeds; every other daemon in the config is \
                                         unaffected",
                            "check" => "whether the daemon is running, whether its socket path \
                                        has moved, and whether an ssh host is reachable",
                        },
                    );
                }
                // Once: a condition that stays true has nothing new to say, and the run log has
                // every attempt after it.
                if attempts.failures() == 1 {
                    raise_problem(&key, Severity::Warning, &never_attached(daemon, &refusal));
                    // A window waiting for this machine to fill it waits no longer: the next one
                    // the config names may fill it now.
                    poison::lock(&ATTACHES, "attaches").failed_once.insert(daemon.id.clone());
                    settle_what_every_window_shows();
                }
                std::thread::sleep(std::time::Duration::from_nanos(retry.after));
                if !attach_current(&daemon.id, ticket) {
                    return;
                }
            }
        }
    }
}

/// Puts back what the arrangement the window opened from had on a daemon that answered after
/// the window opened: its regions, with their widths and the pane each was on, in the places
/// the arrangement had them among whatever the window holds by now (`Saved::keeping`).
///
/// Neither the tab on screen nor the keyboard moves, nor which half of any tab the keyboard
/// would land in. A daemon arriving seconds after launch finds somebody already typing, and
/// moving the keyboard would send what they type next to another machine.
pub(super) fn restore_late(daemon: &DaemonId) {
    {
        let mut guard = poison::lock(&SESSION, "session");
        let session = &mut *guard;
        // Until then its tabs are still arriving, and the ones it has not described yet would
        // lose their place: this runs again when it says it has finished (`restored_from_disk`).
        let restoring = session
            .backends
            .get(daemon)
            .is_some_and(|backend| poison::lock(&backend.mirror, "mirror").restoring());
        if restoring {
            return;
        }
        let mut restored = false;
        for window in session.windows.ids() {
            restored |= restore_late_in(session, window, daemon);
        }
        if !restored {
            return;
        }
    }
    publish("restored_late");
}

/// [`restore_late`] for one window, which may have been waiting on this daemon or not. Says
/// whether it was.
pub(super) fn restore_late_in(session: &mut Session, window: WindowId, daemon: &DaemonId) -> bool {
    let restored = &mut session.windows[window];
    if !restored.awaiting.remove(daemon) {
        return false;
    }
    let Some(left) = restored.left.clone() else { return false };
    if restored.awaiting.is_empty() {
        restored.left = None;
    }
    let described: BTreeSet<TabId> =
        session.backends.get(daemon).map_or_else(BTreeSet::new, |backend| {
            poison::lock(&backend.mirror, "mirror").tabs().map(|tab| tab.id.clone()).collect()
        });
    let restored = &mut session.windows[window];
    let mut regions = 0usize;
    for tab in &left.tabs {
        if !session.holding.holds(&restored.name, &tab.id) || !described.contains(&tab.id) {
            continue;
        }
        for region in tab.regions.iter().filter(|region| &region.daemon == daemon) {
            let Some(id) = restored.composition.open_region(daemon, tab.id.clone()) else {
                continue;
            };
            restored.composition.set_weight(id, region.weight);
            if let Some(pane) = &region.pane {
                restored.composition.set_pane(id, pane.clone());
            }
            regions += 1;
        }
    }
    let wanted = Saved::of(&restored.composition, restored.presentation, &session.font_sizes)
        .keeping(&left, &BTreeSet::from([daemon.clone()]));
    restored.composition.arrange_like(&wanted);
    log::info(
        "composition.restored_late",
        fields! {
            "daemon" => daemon.to_string(),
            "window" => restored.name.to_string(),
            "regions" => regions.to_string(),
        },
    );
    true
}

/// Tells the shell a daemon is being attached, so the title and an empty window can say what
/// the window is waiting for.
///
/// Its own word rather than one of the mirror's health states: those describe a connection
/// that exists, and this daemon has none yet.
pub(super) fn connecting(daemon: &DaemonId) {
    ffi::emit(&Event::new(event::Payload::BackendHealth(connecting_health(daemon))));
}

/// What a daemon being attached is called in a health event.
pub(super) fn connecting_health(daemon: &DaemonId) -> crate::proto::BackendHealth {
    crate::proto::BackendHealth {
        daemon_id: daemon.to_string(),
        state: "connecting".to_string(),
        detail: String::new(),
    }
}

/// What to tell somebody whose daemon, configured or Muster's own, has not attached since launch.
pub(super) fn never_attached(daemon: &Daemon, refusal: &str) -> String {
    let backoff = reconnect::BACKOFF_NS[reconnect::BACKOFF_NS.len() - 1] / 1_000_000_000;
    // Muster's own, with no daemon configured: the only one, and on this machine.
    if named_daemons().is_empty() {
        return format!(
            "Muster could not start its own daemon on this machine: {refusal}. This window has \
             no panes until it does. Muster keeps trying about every {backoff} seconds and the \
             panes arrive on their own once it answers, so relaunching is not necessary."
        );
    }
    format!(
        "Muster could not reach the daemon {}: {refusal}. Its panes are absent from this \
         window, and every other daemon's are unaffected. Muster keeps trying about every {} \
         seconds and its panes arrive on their own once it answers, so relaunching is not \
         necessary. Check that the daemon is running and that the machine it is on is \
         reachable.",
        described(daemon),
        backoff,
    )
}

/// What a daemon that will never attach is said to be, once Muster has stopped trying.
pub(super) fn stopped_attaching(daemon: &Daemon, refusal: &str) -> String {
    let whose = if named_daemons().is_empty() {
        "Muster could not start its own daemon on this machine".to_string()
    } else {
        format!("Muster could not reach the daemon {}", described(daemon))
    };
    format!(
        "{whose}: {refusal}. Waiting will not change that, so Muster has stopped trying, and \
         this daemon's panes are absent from the window until it is fixed and Muster is \
         relaunched. Every other daemon's panes are unaffected."
    )
}

/// Why an attach did not end with the daemon followed.
pub(super) enum Unattached {
    /// The attempt failed, for the reason given, and another can be made.
    Failed(String),
    /// The attempt failed for a reason waiting cannot change, such as a daemon binary that is
    /// not there, so no other is made. Kept to what is certain: a host that does not resolve is
    /// also a laptop whose VPN is down, and that one comes back (kan a_2YAdjHjmh).
    Lasting(String),
    /// The session it was for has been replaced, so nobody wants the daemon any more.
    Abandoned,
}

impl From<String> for Unattached {
    fn from(refusal: String) -> Unattached {
        Unattached::Failed(refusal)
    }
}

/// The daemon binary this app was given, once it is known to be there to run.
pub(super) fn runnable_daemon_binary() -> Result<String, Unattached> {
    let binary = daemon_binary().ok_or_else(|| {
        Unattached::Lasting(
            "this app was not told where its muster-daemon is, so there is no daemon to start and \
             this window will render nothing. A build stages it beside the bridge; this is a bug \
             in how the shell starts Muster."
                .to_string(),
        )
    })?;
    runnable(Path::new(&binary)).map_err(Unattached::Lasting)?;
    Ok(binary)
}

/// Whether a daemon binary is there to run, or why not.
///
/// Asked before launching, because the launch reports a missing file as one more failure to
/// start, and a failure to start is worth trying again where a missing file is not.
pub(super) fn runnable(binary: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(binary) {
        Ok(found) if found.is_file() && found.permissions().mode() & 0o111 != 0 => Ok(()),
        Ok(_) => Err(format!(
            "the muster-daemon at {} is not a program this machine can run",
            binary.display()
        )),
        Err(error) => Err(format!("there is no muster-daemon at {} ({error})", binary.display())),
    }
}

/// How long a window opening onto a daemon waits for its first snapshot before carrying on
/// without it. The daemon's panes arrive on their own when it answers.
pub(super) const FIRST_SNAPSHOT: std::time::Duration = std::time::Duration::from_secs(10);

/// Attaches a daemon for the session there is now, waiting for it: the daemon Muster finds for
/// itself when the config names none, which a window cannot open without.
pub(super) fn attach_daemon(daemon: &Daemon) -> Result<(), String> {
    let ticket = poison::lock(&ATTACHES, "attaches").issue(&daemon.id);
    attach_daemon_in(daemon, ticket).map_err(|unattached| match unattached {
        Unattached::Failed(refusal) | Unattached::Lasting(refusal) => refusal,
        Unattached::Abandoned => "the window was reset while its daemon attached".to_string(),
    })
}

pub(super) fn attach_daemon_in(daemon: &Daemon, ticket: Ticket) -> Result<(), Unattached> {
    let mut reached = reach(&daemon.id, &daemon.endpoint)?;
    let handover = reached.handover.take();
    let socket = reached.socket_path.clone();
    log::info(
        "daemon.attached",
        fields! {
            "daemon" => daemon.id.to_string(),
            "socket" => reached.socket_path.clone(),
            "remote" => reached.tunnel.as_ref().map_or("", Tunnel::host).to_string(),
            "started" => reached.started,
        },
    );
    let connection = {
        let mut session = poison::lock(&SESSION, "session");
        // Asked under the session's lock, which `reset` takes after it moves the generation on,
        // so a daemon either lands in the session that asked for it or not at all.
        if !attach_current(&daemon.id, ticket) {
            return Err(Unattached::Abandoned);
        }
        session.follow(daemon, reached).map_err(Unattached::Failed)?
    };
    // Not under the session's lock: announcing the snapshot takes it.
    if let Some(connection) = connection
        && !connection.wait_for_snapshot(FIRST_SNAPSHOT)
    {
        // Let go of it rather than keep following it. A daemon that never answered is not
        // one this window is showing, and kept, it would count as something followed - so a
        // window whose every configured daemon is silent would open onto nothing instead of
        // saying which of them did not answer. The next attempt reaches it afresh.
        let removed = {
            let mut session = poison::lock(&SESSION, "session");
            if !attach_current(&daemon.id, ticket) {
                return Err(Unattached::Abandoned);
            }
            session.let_go_of(&daemon.id)
        };
        // Dropped with the lock released: dropping a follower joins its thread, which can be
        // part way through connecting, and every other daemon's events need the lock meanwhile.
        drop(removed);
        return Err(Unattached::Failed(format!(
            "the daemon {} did not send its state within {}s",
            daemon.id,
            FIRST_SNAPSHOT.as_secs()
        )));
    }
    if let Some(handover) = handover {
        hand_over_later(&daemon.id, socket, handover);
    }
    Ok(())
}

/// Attaches the daemon on this machine when no config file named any, on a thread of its own
/// like a configured one or while the caller waits, as `how` says.
///
/// Recorded as the wish that produced it - Muster's own daemon, wherever that turns out to be
/// - rather than as the path that answered today.
///
/// A config that named daemons is never answered with this one. Each named daemon is attached
/// on a thread of its own and retried until it answers, so while any is still on its way the
/// window is following something, and this adds nothing. The refusal below is for the one case
/// left: no thread could be started for any of them. Standing in for a daemon somebody named
/// is not a lesser version of showing it: the window renders another session's panes under the
/// configured daemon's id, and nothing on screen says which session it is looking at. Under
/// load that is how the suite's own tests once reached a developer's live herdr (kan a_2L19sAmLZ),
/// and a person whose devenv is briefly slow would get the same window with no way to tell.
pub(super) fn follow_implicitly_if_nothing_else(how: Implicitly) -> Result<(), String> {
    if following_anything() {
        return Ok(());
    }
    let named = named_daemons();
    if !named.is_empty() {
        return Err(format!(
            "none of the daemons the config named is answering: {}. This window has no session \
             behind it, so it renders nothing and ignores the keyboard. A `daemon.unavailable` \
             record in the run log says what each one refused with; the usual causes are a \
             daemon that is not running, a socket path that has moved, and an ssh host that is \
             not reachable.",
            named.join(", ")
        ));
    }
    let implicit =
        Daemon { id: DaemonId::new(LOCAL), endpoint: Endpoint::Local { socket_path: None } };
    match how {
        Implicitly::InBackground => {
            follow_in_background(std::slice::from_ref(&implicit));
            Ok(())
        }
        Implicitly::Waiting => attach_daemon(&implicit),
    }
}

/// Whether a window following the daemon Muster found for itself waits for it.
#[derive(Clone, Copy)]
pub(super) enum Implicitly {
    /// A window opened onto whatever the daemons hold, which opens at once and shows the daemon's
    /// panes when it answers. Its first start after an update can take most of a minute while
    /// macOS checks the new binary, and a window that waited that long read as a Muster that
    /// did not start.
    InBackground,
    /// A window asked for one pane, which has nothing else to show until the daemon holding it
    /// answers, however long its launch takes.
    Waiting,
}

/// Every daemon a config file named, said the way the file named it.
///
/// The endpoint rather than the id, because the id is the reader's own word and the endpoint is
/// the part they can check.
pub(super) fn named_daemons() -> Vec<String> {
    let configured = poison::lock(&startup::CONFIGURED_DAEMONS, "settings");
    configured.as_deref().unwrap_or_default().iter().map(described).collect()
}

/// A daemon said the way the config named it, with the endpoint as well as the id: the id is
/// the reader's own word, and the endpoint is the part they can check.
pub(crate) fn described(daemon: &Daemon) -> String {
    match &daemon.endpoint {
        Endpoint::Local { socket_path: None } => format!("{} on this machine", daemon.id),
        Endpoint::Local { socket_path: Some(path) } => format!("{} at {path}", daemon.id),
        Endpoint::Ssh { host, .. } => format!("{} on {host}", daemon.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A daemon's attach started again from a corrected block is on its first attempt, whatever
    /// the attach it replaces had failed, and that one no longer counts.
    #[test]
    fn a_new_attach_of_a_daemon_is_on_its_first_attempt_and_the_old_one_is_stale() {
        let mut attaches = Attaches {
            generation: 3,
            under_way: BTreeSet::new(),
            latest: BTreeMap::new(),
            issued: 0,
            failed_once: BTreeSet::new(),
        };
        let devenv = DaemonId::new("devenv");
        let first = attaches.issue(&devenv);
        attaches.failed_once.insert(devenv.clone());

        let second = attaches.issue(&devenv);
        assert!(!attaches.failed_once.contains(&devenv), "the new attach has not failed yet");
        assert!(attaches.current(&devenv, second));
        assert!(!attaches.current(&devenv, first), "the attach it replaced still counts");
    }
}
