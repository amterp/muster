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
use muster_core::mirror::{Change, Mirror};
use muster_core::reconnect::Attempts;
use muster_daemon_proto::{self as proto, answer};

use crate::control::{Control, Delivered, Requests};
use crate::convert;
use crate::input::Input;

/// How long a daemon has to answer a request made while connecting.
const PATIENCE: Duration = Duration::from_secs(10);

/// What the follower tells its owner, as it happens. Called with the mirror's lock released,
/// so a reaction that reads the mirror does not wait on the thread that wrote it.
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
    input: Mutex<Option<Input>>,
    /// What the daemon should be told, sent at every connect and whenever it changes.
    settings: Mutex<Option<DaemonSettings>>,
    stopping: AtomicBool,
    /// Which connection is current. A reader ending after its connection was replaced says
    /// nothing about the one that replaced it.
    generation: AtomicU64,
    /// Why the current connection ended, once it has, which is what the follower waits on.
    ended: Mutex<Option<String>>,
    wake: Condvar,
}

impl Connection {
    /// The control connection, while one is open.
    pub fn control(&self) -> Option<Arc<Control>> {
        lock(&self.control).clone()
    }

    /// Sends an input event, or drops it while no input connection is open: nothing is
    /// queued for a daemon that is not there, since keystrokes arriving late are worse than
    /// keystrokes lost.
    pub fn send_input(&self, event: proto::InputEvent) {
        if let Some(input) = lock(&self.input).as_ref() {
            input.send(event);
        }
    }

    fn end(&self, generation: u64, why: String) {
        if generation != self.generation.load(Ordering::Relaxed) {
            return;
        }
        let mut ended = lock(&self.ended);
        ended.get_or_insert(why);
        self.wake.notify_all();
    }
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
        let shared = Arc::clone(&connection);
        let thread = std::thread::Builder::new()
            .name(format!("muster-follow-{}", following.daemon))
            .spawn(move || follow(&following, &shared, &mirror, &notify))?;
        Ok(Follower { connection, thread: Some(thread) })
    }

    pub fn connection(&self) -> Arc<Connection> {
        Arc::clone(&self.connection)
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

impl Drop for Follower {
    fn drop(&mut self) {
        self.connection.stopping.store(true, Ordering::Relaxed);
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
        match connect(following, connection, mirror, notify, &log_position, connected_before) {
            Ok(()) => {
                connected_before = true;
                attempts.holding(monotonic_now());
                let why = wait_for_end(connection);
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
    *lock(&connection.control) = Some(Arc::clone(&control));

    control
        .subscribe()
        .wait(PATIENCE)
        .map_err(|why| format!("the daemon did not answer a subscribe: {why}"))?;

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

    let input = Input::open(&following.socket, &following.client)
        .map_err(|error| format!("could not open an input connection: {error}"))?;
    *lock(&connection.input) = Some(input);
    log::info(
        "daemon.followed",
        fields! {
            "daemon" => following.daemon.clone(),
            "instance" => instance,
            "pid" => control.welcome().pid,
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
            notify(Notice::Bootstrapped { changes, unreadable });
            if std::mem::take(&mut reconnected) {
                notify(Notice::Reconnected);
            }
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

fn wait_for_end(connection: &Connection) -> String {
    let mut ended = lock(&connection.ended);
    loop {
        if let Some(why) = ended.take() {
            return why;
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
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
