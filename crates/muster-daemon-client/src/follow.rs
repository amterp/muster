//! Following one daemon: its connections kept open, its mirror kept current, its log carried
//! into this run's.
//!
//! One thread per daemon holds the connections and reconnects them. What the daemon says is
//! applied on the control connection's own reader thread, in the order it was said, so an
//! answer handed to a request's waiter always follows the events it produced (MIP-3, section
//! 9) - a submit returns with its effect already in the mirror.
//!
//! **A reconnect rebuilds rather than resumes.** A new subscribe answers with the daemon's
//! whole state, so a gap, a daemon that restarted, or one that another daemon replaced are all
//! the same fresh snapshot; the mirror reports only what differs.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use muster_core::daemon_settings::DaemonSettings;
use muster_core::diagnostics::{log, monotonic_now};
use muster_core::fields;
use muster_core::input::NotSent;
use muster_core::mirror::backend::PaneId;
use muster_core::mirror::{Change, Mirror};
use muster_core::reconnect::Attempts;
use muster_daemon_proto::{self as proto, answer};

use crate::control::{Control, Delivered, Requests};
use crate::convert;
use crate::input::Input;

/// How long a daemon has to answer a request made while connecting.
const PATIENCE: Duration = Duration::from_secs(10);

/// What the follower tells its owner, in the order it happened.
///
/// Called on a thread of the follower's own, never the one reading the daemon: a reaction is
/// free to make requests of the same daemon, whose answers that reader has to deliver. The
/// mirror is already current when a notice arrives, and may have moved on since.
pub type Notify = Arc<dyn Fn(Notice) + Send + Sync>;

/// One thing worth telling the window or the run log about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The mirror was rebuilt from a snapshot. Carries what that changed: on a first
    /// connection the whole session, on a reconnect usually nothing. `unreadable` counts tabs
    /// whose tree did not read, which a daemon of this protocol never sends.
    Bootstrapped {
        changes: Vec<Change>,
        unreadable: usize,
    },
    Changed(Change),
    /// The connection ended. The mirror is still the best answer available, and is now a guess
    /// about the present.
    Stale {
        detail: String,
    },
    /// Connected again after a connection had ended.
    Reconnected,
}

/// What to follow, and how to say who is following it.
#[derive(Debug, Clone)]
pub struct Following {
    pub socket: PathBuf,
    /// Who is asking, for the daemon's log.
    pub client: String,
    /// The daemon's name in this window, which its relayed log records carry.
    pub daemon: String,
    /// Whether it is on another machine, whose monotonic clock does not compare with this one's.
    pub remote: bool,
}

/// One daemon's connections as they stand now, shared by everything that talks to it.
#[derive(Debug, Default)]
pub struct Connection {
    control: Mutex<Option<Arc<Control>>>,
    /// A control connection still being set up, which is published as `control` by its own
    /// reader once its snapshot is in the mirror. Held so that letting go of the daemon can hang
    /// it up.
    connecting: Mutex<Option<Arc<Control>>>,
    input: Mutex<Option<Input>>,
    /// What the daemon should be told, sent at every connect and whenever it changes.
    settings: Mutex<Option<DaemonSettings>>,
    stopping: AtomicBool,
    /// Which connection is current. A reader ending after its connection was replaced says
    /// nothing about the one that replaced it.
    generation: AtomicU64,
    /// Why the current connection ended, once it has, which is what the follower waits on.
    ended: Mutex<Option<String>>,
    /// Set when the daemon closed the input connection while the control one stayed up, which
    /// the follower answers by opening another. Guarded by `ended`'s lock, so the follower
    /// cannot miss the wake.
    input_closed: AtomicBool,
    wake: Condvar,
    /// How many snapshots the mirror has been rebuilt from, for a caller waiting on the first.
    snapshots: Mutex<u64>,
    snapshot_arrived: Condvar,
}

impl Connection {
    /// The control connection, while one is open.
    pub fn control(&self) -> Option<Arc<Control>> {
        lock(&self.control).clone()
    }

    /// Queues an input event, or says why it was not: nothing is queued for a daemon that is
    /// not there, since keystrokes arriving late are worse than keystrokes lost.
    pub fn send_input(&self, event: proto::InputEvent) -> Result<(), NotSent> {
        match lock(&self.input).as_ref() {
            Some(input) if input.is_open() => input.send(event),
            _ => Err(NotSent::NotConnected),
        }
    }

    /// Waits up to `patience` for the mirror's first snapshot, and says whether it came.
    pub fn wait_for_snapshot(&self, patience: Duration) -> bool {
        let snapshots = lock(&self.snapshots);
        let (snapshots, _) = self
            .snapshot_arrived
            .wait_timeout_while(snapshots, patience, |count| *count == 0)
            .unwrap_or_else(PoisonError::into_inner);
        *snapshots > 0
    }

    fn end(&self, generation: u64, why: String) {
        if generation != self.generation.load(Ordering::Relaxed) {
            return;
        }
        let mut ended = lock(&self.ended);
        ended.get_or_insert(why);
        self.wake.notify_all();
    }

    fn input_was_closed(&self, generation: u64) {
        if generation != self.generation.load(Ordering::Relaxed) {
            return;
        }
        let _ended = lock(&self.ended);
        self.input_closed.store(true, Ordering::Relaxed);
        self.wake.notify_all();
    }
}

/// Opens an input connection that tells `connection` when the daemon closes it.
fn open_input(following: &Following, connection: &Arc<Connection>) -> Result<Input, String> {
    let generation = connection.generation.load(Ordering::Relaxed);
    let told = Arc::downgrade(connection);
    let on_closed = Box::new(move || {
        if let Some(connection) = told.upgrade() {
            connection.input_was_closed(generation);
        }
    });
    Input::open(&following.socket, &following.client, on_closed)
        .map_err(|error| format!("could not open an input connection: {error}"))
}

/// A daemon being followed. Dropping it hangs up and stops following.
#[derive(Debug)]
pub struct Follower {
    connection: Arc<Connection>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Follower {
    pub fn start(
        following: Following,
        mirror: Arc<Mutex<Mirror>>,
        notify: Notify,
    ) -> std::io::Result<Follower> {
        let connection = Arc::new(Connection::default());
        let notify = notifier(&following.daemon, &connection, notify)?;
        let shared = Arc::clone(&connection);
        let thread = std::thread::Builder::new()
            .name(format!("muster-follow-{}", following.daemon))
            .spawn(move || follow(&following, &shared, &mirror, &notify))?;
        Ok(Follower { connection, thread: Some(thread) })
    }

    pub fn connection(&self) -> Arc<Connection> {
        Arc::clone(&self.connection)
    }

    /// Tells the daemon a window with the keyboard is showing these panes, and says whether it
    /// was sent. Not held for a later connection: what a window is showing then is reported
    /// again when it comes back (`muster_core::attention`).
    pub fn seen(&self, panes: &[PaneId]) -> bool {
        let Some(control) = self.connection.control() else {
            return false;
        };
        control.seen(panes.iter().map(ToString::to_string).collect());
        true
    }

    /// Tells the daemon these settings now, if connected, and at every connect after. Only
    /// what differs from the last settings is sent.
    pub fn configure(&self, settings: &DaemonSettings) {
        let previous = lock(&self.connection.settings).replace(settings.clone());
        if let Some(control) = self.connection.control() {
            send_settings(&control, previous.as_ref(), settings);
        }
    }
}

/// Hands notices to `notify` on a thread of their own, in order (see [`Notify`]).
///
/// Not joined when the follower is dropped: its owner may be holding whatever a reaction is
/// waiting for. It says nothing once the follower is stopping, and ends when the last sender
/// goes with the threads that hold one.
fn notifier(daemon: &str, connection: &Arc<Connection>, notify: Notify) -> std::io::Result<Notify> {
    let (tell, told) = std::sync::mpsc::channel::<Notice>();
    let connection = Arc::clone(connection);
    std::thread::Builder::new().name(format!("muster-notice-{daemon}")).spawn(move || {
        for notice in told {
            if connection.stopping.load(Ordering::Relaxed) {
                return;
            }
            notify(notice);
        }
    })?;
    Ok(Arc::new(move |notice| {
        let _ = tell.send(notice);
    }))
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.connection.stopping.store(true, Ordering::Relaxed);
        if let Some(connecting) = lock(&self.connection.connecting).take() {
            connecting.hang_up();
        }
        lock(&self.connection.control).take();
        lock(&self.connection.input).take();
        let current = self.connection.generation.load(Ordering::Relaxed);
        self.connection.end(current, "this window stopped following the daemon".to_string());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Where following the daemon's log has got to: the daemon run, and the last record carried.
#[derive(Debug, Default, Clone, Copy)]
struct LogPosition {
    instance: u64,
    last: Option<u64>,
}

fn follow(
    following: &Following,
    connection: &Arc<Connection>,
    mirror: &Arc<Mutex<Mirror>>,
    notify: &Notify,
) {
    let log_position = Arc::new(Mutex::new(LogPosition::default()));
    let mut attempts = Attempts::new();
    let mut connected_before = false;
    while !connection.stopping.load(Ordering::Relaxed) {
        connection.generation.fetch_add(1, Ordering::Relaxed);
        *lock(&connection.ended) = None;
        connection.input_closed.store(false, Ordering::Relaxed);
        match connect(following, connection, mirror, notify, &log_position, connected_before) {
            Ok(()) => {
                connected_before = true;
                attempts.holding(monotonic_now());
                let why = hold(following, connection);
                attempts.holding(monotonic_now());
                lock(&connection.control).take();
                lock(&connection.input).take();
                if connection.stopping.load(Ordering::Relaxed) {
                    return;
                }
                mirror.lock().unwrap_or_else(PoisonError::into_inner).mark_stale(&why);
                notify(Notice::Stale { detail: why });
            }
            Err(why) => {
                lock(&connection.control).take();
                lock(&connection.input).take();
                let mut held = mirror.lock().unwrap_or_else(PoisonError::into_inner);
                if connected_before {
                    held.mark_stale(&why);
                } else {
                    held.mark_disconnected(&why);
                }
                drop(held);
                notify(Notice::Stale { detail: why });
            }
        }
        let retry = attempts.failed();
        if retry.report {
            log::warn(
                "daemon.follow.failing",
                fields! {
                    "daemon" => following.daemon.clone(),
                    "attempts" => retry.attempt,
                    "impact" => "this daemon's panes show what they last showed, and requests to \
                                 it fail until it answers again",
                    "check" => "whether the daemon is running (its log beside its socket), and \
                                for a remote one whether the ssh connection is up",
                },
            );
        }
        sleep_unless_stopping(connection, Duration::from_nanos(retry.after));
    }
}

/// Opens the connections, and returns once the daemon is followed.
fn connect(
    following: &Following,
    connection: &Arc<Connection>,
    mirror: &Arc<Mutex<Mirror>>,
    notify: &Notify,
    log_position: &Arc<Mutex<LogPosition>>,
    reconnecting: bool,
) -> Result<(), String> {
    let deliver = delivery(following, connection, mirror, notify, log_position, reconnecting);
    let control = Arc::new(
        Control::open(&following.socket, &following.client, deliver)
            .map_err(|error| format!("could not open a control connection: {error}"))?,
    );
    let instance = control.welcome().instance;
    *lock(&connection.connecting) = Some(Arc::clone(&control));
    if connection.stopping.load(Ordering::Relaxed) {
        return Err("this window stopped following the daemon".to_string());
    }

    let subscribed = control.subscribe().wait(PATIENCE);
    // Published by the snapshot's delivery, unless the subscribe failed.
    lock(&connection.connecting).take();
    let subscribed =
        subscribed.map_err(|why| format!("the daemon did not answer a subscribe: {why}"))?;
    if !matches!(subscribed.detail, Some(answer::Detail::Snapshot(_))) {
        return Err(format!(
            "the daemon answered a subscribe without its state ({})",
            subscribed.reason
        ));
    }

    // After the snapshot, so the run's log reads connect, state, then the daemon's side. A
    // daemon run this window has not followed before is followed from as far back as it holds.
    let after = {
        let mut position = lock(log_position);
        if position.instance != instance {
            *position = LogPosition { instance, last: None };
        }
        position.last
    };
    let followed = control.follow_log(after).wait(PATIENCE);
    if let Ok(proto::Answer { detail: Some(answer::Detail::Followed(followed)), .. }) = &followed
        && let Some(after) = after
        && followed.oldest > after + 1
    {
        log::warn(
            "daemon.log.gap",
            fields! {
                "daemon" => following.daemon.clone(),
                "wanted_from" => after + 1,
                "oldest_held" => followed.oldest,
                "impact" => "the daemon's records between these two are not in this run's log",
                "check" => "the daemon's own log file beside its socket, which has them",
            },
        );
    }

    let settings = lock(&connection.settings).clone();
    if let Some(settings) = settings {
        send_settings(&control, None, &settings);
    }

    *lock(&connection.input) = Some(open_input(following, connection)?);
    log::info(
        "daemon.followed",
        fields! {
            "daemon" => following.daemon.clone(),
            "instance" => instance,
            "daemon_pid" => control.welcome().pid,
            "version" => control.welcome().daemon_version.clone(),
        },
    );
    Ok(())
}

/// What the control connection's reader does with each thing the daemon says.
fn delivery(
    following: &Following,
    connection: &Arc<Connection>,
    mirror: &Arc<Mutex<Mirror>>,
    notify: &Notify,
    log_position: &Arc<Mutex<LogPosition>>,
    reconnecting: bool,
) -> impl FnMut(Delivered, &Requests) + Send + 'static {
    let mirror = Arc::clone(mirror);
    let notify = Arc::clone(notify);
    let log_position = Arc::clone(log_position);
    let daemon = following.daemon.clone();
    let remote = following.remote;
    let connection = Arc::clone(connection);
    let generation = connection.generation.load(Ordering::Relaxed);
    let mut reconnected = reconnecting;
    move |delivered, requests| match delivered {
        Delivered::Subscribed(snapshot) => {
            let (snapshot, unreadable) = convert::snapshot(*snapshot);
            let changes = mirror.lock().unwrap_or_else(PoisonError::into_inner).bootstrap(snapshot);
            // Published here, between the snapshot and the notice: a request made before it is
            // refused as not connected, where it could be answered while the mirror still held
            // the picture from before, and one made in answer to the notice finds it connected.
            if connection.generation.load(Ordering::Relaxed) == generation
                && let Some(control) = lock(&connection.connecting).take()
            {
                *lock(&connection.control) = Some(control);
            }
            notify(Notice::Bootstrapped { changes, unreadable });
            if std::mem::take(&mut reconnected) {
                notify(Notice::Reconnected);
            }
            *lock(&connection.snapshots) += 1;
            connection.snapshot_arrived.notify_all();
        }
        Delivered::Event(event) => {
            let Some(event) = convert::event(*event) else { return };
            let changes = mirror.lock().unwrap_or_else(PoisonError::into_inner).apply(event);
            for change in changes {
                notify(Notice::Changed(change));
            }
        }
        // The snapshot that answers says where things stand, and arrives as `Subscribed`.
        Delivered::Gap { .. } => requests.subscribe(),
        Delivered::Log(line) => {
            let received = remote.then(monotonic_now);
            log::relay(&line.line, &daemon, received);
            lock(&log_position).last = Some(line.number);
        }
        // How the follower learns to reconnect.
        Delivered::Ended(why) => connection.end(generation, why),
    }
}

/// Keeps a followed daemon's input open until the connection ends, and returns why it did.
fn hold(following: &Following, connection: &Arc<Connection>) -> String {
    loop {
        match wait_for_end(connection) {
            Woke::Ended(why) => return why,
            Woke::InputClosed => match open_input(following, connection) {
                Ok(input) => {
                    *lock(&connection.input) = Some(input);
                    log::info(
                        "daemon.input.reopened",
                        fields! { "daemon" => following.daemon.clone() },
                    );
                }
                // A daemon that takes a control connection and no input one is not usable, so
                // the whole connection is made again.
                Err(why) => return why,
            },
        }
    }
}

enum Woke {
    Ended(String),
    InputClosed,
}

fn wait_for_end(connection: &Connection) -> Woke {
    let mut ended = lock(&connection.ended);
    loop {
        if let Some(why) = ended.take() {
            return Woke::Ended(why);
        }
        if connection.input_closed.swap(false, Ordering::Relaxed) {
            return Woke::InputClosed;
        }
        ended = connection.wake.wait(ended).unwrap_or_else(PoisonError::into_inner);
    }
}

fn sleep_unless_stopping(connection: &Connection, wait: Duration) {
    let ended = lock(&connection.ended);
    let _ = connection
        .wake
        .wait_timeout_while(ended, wait, |_| !connection.stopping.load(Ordering::Relaxed));
}

/// Sends each setting that differs from `previous`, or every one when there is none. Answers
/// are not waited for: a refusal is the daemon's to log, and the next connect sends them again.
fn send_settings(control: &Control, previous: Option<&DaemonSettings>, settings: &DaemonSettings) {
    if previous.is_none_or(|previous| previous.shell != settings.shell) {
        control.set_shell(convert::shell(settings));
    }
    if previous.is_none_or(|previous| previous.scrollback_bytes != settings.scrollback_bytes) {
        control.set_scrollback(settings.scrollback_bytes);
    }
    if previous.is_none_or(|previous| previous.palette != settings.palette)
        && let Some(palette) = &settings.palette
    {
        control.set_palette(convert::palette(palette));
    }
    if previous.is_none_or(|previous| previous.cursor != settings.cursor) {
        control.set_cursor(convert::cursor(settings));
    }
    if previous.is_none_or(|previous| previous.clipboard_write != settings.clipboard_write) {
        control.set_clipboard_write(settings.clipboard_write.allowed());
    }
    if previous.is_none_or(|previous| {
        previous.scroll_multiplier.to_bits() != settings.scroll_multiplier.to_bits()
    }) {
        control.set_scroll_multiplier(settings.scroll_multiplier);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use muster_harness::requests::{create, in_new_tab, make, read_text, until_text};
    use muster_harness::{Daemon, until};

    /// The daemon closes an input connection it cannot read and expects the client to open
    /// another; until one opens, every pane on the daemon ignores the keyboard.
    #[test]
    fn input_the_daemon_closed_is_opened_again() {
        let daemon = Daemon::start_built();
        let mut control = daemon.connect();
        make(&mut control, create("p1", in_new_tab("t1")));
        until_text(&mut control, "p1", "$");
        let follower = Follower::start(
            Following {
                socket: daemon.socket_path().to_path_buf(),
                client: "test".to_string(),
                daemon: "local".to_string(),
                remote: false,
            },
            Arc::new(Mutex::new(Mirror::new())),
            Arc::new(|_| {}),
        )
        .unwrap();
        let connection = follower.connection();
        assert!(connection.wait_for_snapshot(PATIENCE));
        until("the input connection to open", || lock(&connection.input).is_some(), ());

        lock(&connection.input).as_ref().unwrap().break_underneath();
        let typed = |text: &str| proto::InputEvent {
            pane: "p1".into(),
            input: Some(proto::input_event::Input::Send(proto::input_event::Send {
                text: text.into(),
                enter: true,
            })),
        };
        // Typed until it shows, as a person would: what went before the writer found the
        // break is lost with the connection it was queued on.
        until(
            "typing to reach the pane again",
            || {
                let _ = connection.send_input(typed("echo REOPENED"));
                read_text(&mut control, "p1", 0, 0).text.contains("REOPENED\n")
            },
            (),
        );
    }
}
