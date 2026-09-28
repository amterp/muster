//! What this process is holding open: daemons, their mirrors, and the attached panes.
//!
//! The runtime half of composition. Which daemons are attached and what each region shows
//! is a record in the core, judged by `composition.json` with no socket in sight; what is
//! here is the part that genuinely needs one - a follower per daemon, a bound socket per pane,
//! and the threads behind both.
//!
//! Keyed by daemon and by pane throughout, because both are plural: one window can show a
//! laptop and a devenv side by side. When daemons named their own panes, two of them both
//! handed out `w1:p1`; a pane's name is Muster's now and unique across machines, and the daemon
//! in the key is what says which machine to ask.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use muster_core::AgentState;
use muster_core::attention::{Attend, Attention, Notifications};
use muster_core::composition::{
    Composition, Daemon, DaemonId, Endpoint, FontSizeChange, FontSizes, Frame, HeldWindow,
    MusterTab, PaneKey, Presentation, RegionId, Saved, Step, View, ViewPane, WindowName, saved,
    zoom_filling,
};
use muster_core::config::{Appearance, Config, Feel};
use muster_core::daemon_settings::DaemonSettings;
use muster_core::diagnostics::{clock, log, poison};
use muster_core::equalize::{self, Evenly};
use muster_core::fields;
use muster_core::input::{Bindings, InputEvent, InputSink, PaneInput, PaneInputSettings};
use muster_core::intent::{BackendChannel, BackendIntent, MoveDestination, Outcome, Refusal};
use muster_core::mirror::backend::{PaneId, TabId};
use muster_core::mirror::{Change, Health, Mirror, Restored};
use muster_core::names::Minter;
use muster_core::pane_focus::PaneFocus;
use muster_core::pane_text::PaneText;
use muster_core::problems::{Problem, Problems, Severity};
use muster_core::reconnect;
use muster_core::respawn::{self, Decision, Ended, Ending, Respawns};
use muster_core::roster::{Numbering, Roster, RosterTab, TabStep};
use muster_daemon_client::backend::{DaemonBackend, DaemonInput};
use muster_daemon_client::follow::{Follower, Following, Notice};
use muster_daemon_client::{environment, install as remote_install, launch, records, remote};
use muster_daemon_proto::install;
use muster_ssh::{Forward, State as TunnelState, Tunnel, remote_environment};

use crate::bridge_link::{PaneLink, Reports};
use crate::holding::Holding;
use crate::proto::{
    AttentionChanged, ClipboardWrite, Event, PaneTypeable, PasteHeld, PresentationChanged,
    Problem as ProblemMessage, ProblemsChanged, RaiseWindow, ReopenWindow, RosterChanged,
    ViewChanged, event,
};
use crate::watch::{self, Seen};
use crate::{command, convert, ffi, watchdog};

/// What Muster calls the daemon it found for itself.
///
/// The name for the one nobody named. A config file that lists daemons names its own, and
/// this is what a config-less Muster calls the daemon on this machine.
const LOCAL: &str = "local";

/// The daemon binary this Muster ships, as the shell resolved it.
///
/// Held here rather than looked up, because where it sits is an OS and packaging question -
/// inside a bundle for a shipped app, beside the binary for a build - and the core answers
/// none of those. The shell hands it over at startup, the way it already does the log file
/// and the config file.
///
/// None means the shell found none, which is a real state and not a default to paper over: a
/// window with no daemon to start says so rather than rendering nothing in silence.
static DAEMON_BINARY: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_daemon_binary(path: &str) {
    let mut held = poison::lock(&DAEMON_BINARY, "daemon-binary");
    *held = if path.is_empty() { None } else { Some(path.to_string()) };
}

fn daemon_binary() -> Option<String> {
    poison::lock(&DAEMON_BINARY, "daemon-binary").clone()
}

/// The daemon's data directory, when the shell keeps it somewhere other than beside the
/// binary. None is the directory beside it, which is where a build puts it.
static DAEMON_DATA: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_daemon_data(path: &str) {
    let mut held = poison::lock(&DAEMON_DATA, "daemon-data");
    *held = if path.is_empty() { None } else { Some(path.to_string()) };
}

/// Where the shell keeps the Linux daemons it can install on another machine: a directory
/// holding one per architecture. None is a build that carries none.
static REMOTE_DAEMONS: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_remote_daemons(path: &str) {
    let mut held = poison::lock(&REMOTE_DAEMONS, "remote-daemons");
    *held = if path.is_empty() { None } else { Some(path.to_string()) };
}

/// Every daemon this app can put on another machine, and the files that go with them.
///
/// A remote Mac runs this Mac's own daemon, so it is sent that, with the libghostty-vt this
/// process loaded, which is the one the daemon was built against.
fn carried() -> remote_install::Carried {
    let mac = daemon_binary().map(PathBuf::from);
    let data = poison::lock(&DAEMON_DATA, "daemon-data").clone().map(PathBuf::from).or_else(|| {
        mac.as_deref().and_then(Path::parent).map(|beside| beside.join("muster-daemon-data"))
    });
    remote_install::Carried {
        linux: poison::lock(&REMOTE_DAEMONS, "remote-daemons").clone().map(PathBuf::from),
        mac: mac.filter(|_| cfg!(all(target_os = "macos", target_arch = "aarch64"))),
        mac_library: muster_vt::library_path(),
        data,
    }
}

/// What locale this machine is set to, as the shell read it off the platform.
///
/// Held for the same reason the daemon binary is: only the shell can ask macOS what the user
/// picked, and only the core decides what a daemon is entitled to. None means the platform
/// would not name one, and nothing is invented in its place.
static PLATFORM_LOCALE: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_platform_locale(locale: &str) {
    let mut held = poison::lock(&PLATFORM_LOCALE, "locale");
    *held = if locale.is_empty() { None } else { Some(locale.to_string()) };
}

fn platform_locale() -> Option<String> {
    poison::lock(&PLATFORM_LOCALE, "locale").clone()
}

/// The directory Muster keeps its own commands in, for the daemons it starts.
///
/// Held here rather than asked for at each start, on the same terms as the locale above: it is a
/// question only the shell can answer, and it is needed in the middle of attaching a daemon.
///
/// None means this build has no CLI to offer, and then a daemon's PATH is left exactly as it was
/// inherited - a pane simply has no `muster` in it.
static COMMANDS: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_commands_path(path: &str) {
    let mut held = poison::lock(&COMMANDS, "commands");
    *held = if path.is_empty() { None } else { Some(path.to_string()) };
}

fn commands_path() -> Option<String> {
    poison::lock(&COMMANDS, "commands").clone()
}

/// Where Muster writes down the daemons it starts.
///
/// Held on the same terms as the three above. Needed at two moments that are nowhere near each
/// other: when a daemon is started, and when somebody asks what is on this machine.
///
/// None means the shell found nowhere to write, and then `muster daemons` answers with nothing
/// and says that is why - "no record" and "no daemons" are opposite things to tell somebody
/// about to end a process.
static DAEMON_RECORDS: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_daemon_records_path(path: &str) {
    let mut held = poison::lock(&DAEMON_RECORDS, "daemon-records");
    *held = if path.is_empty() { None } else { Some(path.to_string()) };
}

pub(crate) fn daemon_records_path() -> Option<String> {
    poison::lock(&DAEMON_RECORDS, "daemon-records").clone()
}

/// Where this window's arrangement is remembered, and what was last written there.
///
/// The text rather than the record, so that deciding whether to write is a string compare
/// against what is actually on disk. Composition settles on every publish and publishes
/// happen on every agent transition, so most of them have nothing to save.
///
/// None means remember nothing, which is what a shell that found nowhere to write says and
/// what every test that never sets one gets.
static STATE: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Whether this window has worked out what it is showing.
///
/// A window with nothing on screen means two different things either side of this flag, and
/// both readers below turn on the difference. Before it, the composition is empty because
/// nobody has decided yet and [`open`] is about to; after it, empty is an answer.
///
/// Nothing writes the arrangement before this is true. A composition nobody has opened yet is
/// empty, and an empty one saved over the file is a window that comes back with no tabs at
/// all - the exact loss the file exists to prevent. It is not hypothetical: the shell reports
/// its frame as soon as the window has one, which is before it asks the core to open anything,
/// so without this a launch would blank the arrangement it was about to restore.
///
/// Nothing opens a region before it either. The daemons are followed on one request and the
/// window is opened on another, and the app builds a renderer, a menu and a window in between,
/// so a daemon's first bootstrap lands in that gap - and the standing rule that a daemon with
/// nothing on screen gets a region would answer it there, before the saved arrangement has
/// been read. The restore then added its own region onto the same tab, which is a pane drawn
/// twice and a bridge that cannot attach.
///
/// It also keeps `--renderer-check` from overwriting somebody's arrangement with the empty
/// window it deliberately opens.
static OPENED: AtomicBool = AtomicBool::new(false);

/// Says the window now knows what it is showing, so the arrangement may be written.
fn mark_opened() {
    OPENED.store(true, Ordering::Relaxed);
}

fn opened() -> bool {
    OPENED.load(Ordering::Relaxed)
}

/// Which chord asks for which action, as the config file left it.
///
/// Held rather than passed, for the reason the daemon binary and the state path are: a shell
/// asks for these once at launch, and threading them through every caller in between would be
/// a parameter nothing else in that path uses.
static BINDINGS: Mutex<Option<Bindings>> = Mutex::new(None);

pub(crate) fn set_bindings(bindings: Bindings) {
    *poison::lock(&BINDINGS, "bindings") = Some(bindings);
}

/// The bindings in force, which with no config file is what Muster ships.
pub(crate) fn bindings() -> Bindings {
    poison::lock(&BINDINGS, "bindings").clone().unwrap_or_default()
}

/// What the config file said about typing, held for the panes attached after it was read.
///
/// Beside [`BINDINGS`] and for the same reason: a pane is attached from several places and
/// none of them has a config file in hand.
///
/// Read at attach rather than per keystroke, so a pane keeps the settings it was attached
/// with. That is what makes a change need a relaunch, and it is the honest arrangement while
/// the encoder is built once per pane - re-reading here would leave a window whose panes
/// disagree depending on when each was opened.
static PANE_INPUT: Mutex<Option<PaneInputSettings>> = Mutex::new(None);

pub(crate) fn set_pane_input(settings: PaneInputSettings) {
    *poison::lock(&PANE_INPUT, "input-settings") = Some(settings);
}

/// The typing settings in force, which with no config file is what Muster ships.
pub(crate) fn pane_input() -> PaneInputSettings {
    poison::lock(&PANE_INPUT, "input-settings").clone().unwrap_or_default()
}

/// What each daemon is told about this window's settings (`muster_core::daemon_settings`),
/// held for daemons reached later and sent to every one followed now whenever it changes.
static SETTINGS: Mutex<Option<DaemonSettings>> = Mutex::new(None);

pub(crate) fn set_daemon_settings(settings: DaemonSettings) {
    // Stored under the session's lock, which `follow` holds while it reads these: a daemon
    // reached in between is told either by the loop here or by the value it reads.
    let session = poison::lock(&SESSION, "session");
    for backend in session.backends.values() {
        backend.follower.configure(&settings);
    }
    *poison::lock(&SETTINGS, "daemon-settings") = Some(settings);
}

/// The root knobs, held for whatever asks about them next.
///
/// Beside [`BINDINGS`] and [`PANE_INPUT`], for the same reason: a resize arrives from a
/// keystroke, and that caller has no config file in hand.
static FEEL: Mutex<Option<Feel>> = Mutex::new(None);

pub(crate) fn set_feel(feel: Feel) {
    *poison::lock(&FEEL, "settings") = Some(feel);
}

/// The knobs in force, which with no config file is what Muster ships.
pub(crate) fn feel() -> Feel {
    poison::lock(&FEEL, "settings").unwrap_or_default()
}

/// The config file this run was started with, so a reload knows what to read again.
///
/// Held rather than re-derived: where the file lives is the shell's answer, given once at
/// startup, and a core that went looking for one itself would be a second answer to a question
/// it does not own.
static CONFIG_PATH: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_config_path(path: &str) {
    *poison::lock(&CONFIG_PATH, "settings") = Some(path.to_string());
}

/// The file to read again, or empty when this run was started without one.
pub(crate) fn config_path() -> String {
    poison::lock(&CONFIG_PATH, "settings").clone().unwrap_or_default()
}

/// What the window should look like, held the same way and for the same reason.
///
/// Cloned on read rather than copied, because a palette and a font family are not `Copy`. It
/// is read once at launch by a shell standing up its renderer, so the cost is a font name and
/// sixteen colours, once.
static APPEARANCE: Mutex<Option<Appearance>> = Mutex::new(None);

pub(crate) fn set_appearance(appearance: Appearance) {
    *poison::lock(&APPEARANCE, "settings") = Some(appearance);
}

/// The appearance in force, which with no config file is every value absent - so the renderer
/// paints what it would have painted anyway.
pub(crate) fn appearance() -> Appearance {
    poison::lock(&APPEARANCE, "settings").clone().unwrap_or_default()
}

/// Takes the file's answer about which agent states are worth interrupting somebody for.
///
/// Held on `Attention` rather than beside the other settings, because it is not a value
/// anything reads back - it decides what joins the unread set, and the unread set is there.
///
/// Anything the new answer silences is taken down, which is what makes a mute mean quiet
/// rather than "no new ones". Nothing is raised the other way: see `Attention::notifying`.
pub(crate) fn set_notifications(notifications: Notifications) {
    let stale = {
        let mut session = poison::lock(&SESSION, "session");
        session.attention.notifying(notifications)
    };
    for pane in &stale {
        announce_attention(pane, Attend::Withdrawn);
    }
}

/// What is wrong with this window, and whether Muster opened the roster to say so.
///
/// Beside the settings rather than in `SESSION` because a problem is not part of an
/// arrangement: nothing here is written to `window.toml` and nothing survives a launch. A
/// config still broken on the next launch is raised again by reading it again, which is the
/// only answer that cannot go stale.
static PROBLEMS: Mutex<Option<ProblemState>> = Mutex::new(None);

/// Panes whose bridge is known to be gone, and whose replacement has not dialed yet.
///
/// Two things watch a bridge die, and either may arrive first: the link socket this window
/// bound for the pane, which is the one that reliably does, and the renderer reporting that a
/// surface's command exited. The second arrival is not a second death.
///
/// A leaf lock: nothing is called while it is held, so it can be taken from a call site that
/// already holds `SESSION` and from one that holds nothing without an ordering to remember.
static DARK: Mutex<BTreeSet<PaneKey>> = Mutex::new(BTreeSet::new());

#[derive(Default)]
struct ProblemState {
    problems: Problems,

    /// True when Muster opened the roster itself to show an error, having found it closed.
    /// Kept so that clearing the last error can put it back the way somebody left it -
    /// borrowing the roster is defensible, keeping it is not.
    opened_sidebar: bool,
}

/// Records that something is wrong, and makes sure somebody can see it.
///
/// Everything a caller has to remember is in here on purpose. Publishing only on a real
/// change is what stops a file saved repeatedly with one typo from reopening a roster
/// somebody keeps closing, and doing the roster and the event together is what stops one
/// from being forgotten at a new call site.
pub(crate) fn raise_problem(key: &str, severity: Severity, detail: &str) {
    let changed = {
        let mut held = poison::lock(&PROBLEMS, "problems");
        held.get_or_insert_with(ProblemState::default).problems.raise(key, severity, detail)
    };
    if !changed {
        return;
    }
    // The sentence a person actually read, which the run log otherwise lacked: the conditions
    // that raise a problem mostly log themselves, and the watches on panes log nothing, so the
    // one line an incident turned on had to be inferred from the events around it (kan
    // a_2LMpvavhA). Info rather than a level taken from the severity, because the condition's
    // own record carries the level where there is one.
    log::info(
        "problem.raised",
        fields! { "key" => key, "severity" => severity.as_str(), "detail" => detail },
    );
    reconcile_sidebar_with_problems();
    announce_problems();
}

/// Records that something is no longer wrong, and why.
///
/// Called from every success path, including the ones where nothing was ever wrong, so the
/// common case is a call that changes nothing and says nothing.
///
/// `why` goes to the run log, because a problem going away is not always the thing it was about
/// being fixed. A painting warning cleared by the pane painting and one cleared because nobody
/// could see the pane any more used to be the same record, and only one of them meant the pane
/// was working (kan a_2LWqtPd8E).
pub(crate) fn clear_problem(key: &str, why: &str) {
    let changed = {
        let mut held = poison::lock(&PROBLEMS, "problems");
        held.get_or_insert_with(ProblemState::default).problems.clear(key)
    };
    if !changed {
        return;
    }
    log::info("problem.cleared", fields! { "key" => key, "why" => why });
    reconcile_sidebar_with_problems();
    announce_problems();
}

/// Everything wrong with this window, worst first.
pub(crate) fn problems() -> Vec<Problem> {
    poison::lock(&PROBLEMS, "problems")
        .as_ref()
        .map(|held| held.problems.outstanding())
        .unwrap_or_default()
}

/// Makes the roster's visibility agree with whether an error is outstanding.
///
/// Derived rather than decided at the moment a problem arrives, because the two inputs land in
/// an order nothing guarantees. A config refused during `Startup` raises its problem before
/// `open()` has restored whether the roster was open at all, so a version of this that checked
/// once at raise time checked a default that was about to be replaced - and opened nothing, for
/// a window that came back with the roster put away and a broken config. Reconciling from both
/// sides makes the order stop mattering.
///
/// Only errors open a roster. A warning in a list somebody will look at eventually is fine, and
/// reflowing every pane to mention a stale daemon would be worse than staying quiet. Reflowing
/// is the real cost here and it is why the line is drawn: somebody typing when their config
/// breaks gets their panes resized underneath them, which is accepted, because the alternative
/// is the silence that cost an evening.
/// Answers whether it moved the roster, so a caller mid-announcement does not say it twice.
fn reconcile_sidebar_with_problems() -> bool {
    let error =
        poison::lock(&PROBLEMS, "problems").as_ref().is_some_and(|held| held.problems.has_error());
    let shown = poison::lock(&SESSION, "session").presentation.sidebar;

    if error {
        if shown {
            return false;
        }
        poison::lock(&PROBLEMS, "problems")
            .get_or_insert_with(ProblemState::default)
            .opened_sidebar = true;
        log::info(
            "problems.sidebar.opened",
            fields! {
                "impact" => "the roster was closed and an error would have had nowhere to \
                             appear, so Muster opened it",
                "check" => "it closes again on its own when the last error clears, unless you \
                            open or close it yourself first",
            },
        );
        set_sidebar(true);
        return true;
    }

    // Borrowed, so give it back. Only when Muster was the one who opened it: a roster somebody
    // opened themselves is theirs, and closing it because a problem happened to clear would be
    // Muster tidying away a window it does not own.
    let borrowed = {
        let mut held = poison::lock(&PROBLEMS, "problems");
        let held = held.get_or_insert_with(ProblemState::default);
        let borrowed = held.opened_sidebar;
        held.opened_sidebar = false;
        borrowed
    };
    if borrowed && shown {
        log::info(
            "problems.sidebar.closed",
            fields! {
                "impact" => "the last error cleared, so the roster Muster opened to show it \
                             has been put back the way it was found",
            },
        );
        set_sidebar(false);
        return true;
    }
    false
}

/// Shows or puts away the roster, without asking what it was.
///
/// The half of [`toggle_sidebar`] that is not the toggle. Split out so a problem can open the
/// roster without a second copy of "write it, tell the shell, save it" - which is exactly the
/// kind of second copy that ends up forgetting the save.
fn set_sidebar(shown: bool) {
    let presentation = {
        let mut session = poison::lock(&SESSION, "session");
        if session.presentation.sidebar == shown {
            return;
        }
        session.presentation = session.presentation.with_sidebar(shown);
        session.presentation
    };
    announce_presentation(presentation);
    publish("sidebar");
}

fn announce_problems() {
    let problems = problems();
    ffi::emit(&Event {
        payload: Some(event::Payload::ProblemsChanged(ProblemsChanged {
            problems: problems
                .into_iter()
                .map(|problem| ProblemMessage {
                    key: problem.key,
                    severity: problem.severity.as_str().to_string(),
                    detail: problem.detail,
                })
                .collect(),
        })),
    });
}

pub(crate) fn set_state_path(path: &str) {
    let mut held = poison::lock(&STATE, "saved-arrangement");
    *held = if path.is_empty() { None } else { Some((path.to_string(), String::new())) };
}

/// Says where every window writes which window holds each tab, and which window this is.
///
/// Named after its arrangement, so a window that is reopened is the same window and comes back
/// to its tabs (MIP-2). The socket is how other windows tell whether this one is open.
///
/// On the session rather than in a static, because it describes this launch: a test that reset
/// the statics and not this would open its window as the last test's.
pub(crate) fn set_tab_holders(record: &str, arrangement: &str, socket: &str) {
    poison::lock(&SESSION, "session").holding = Holding::new(record, arrangement, socket);
}

/// A daemon found or started, and how it is reached.
#[derive(Debug)]
struct Reached {
    socket_path: String,
    tunnel: Option<Tunnel>,
    /// Whether Muster started it, rather than finding it already answering.
    started: bool,
}

/// What every pane this window makes is handed beyond what its daemon gives it: the window's own
/// socket, so a program in the pane can drive the window it is drawn in. The daemon gives the
/// pane its name (`MUSTER_PANE`) itself, from the request that makes it.
fn pane_environment() -> BTreeMap<String, String> {
    command::listening_at()
        .map(|socket| BTreeMap::from([(environment::WINDOW_SOCKET.to_string(), socket)]))
        .unwrap_or_default()
}

fn reach(daemon: &DaemonId, endpoint: &Endpoint) -> Result<Reached, String> {
    match endpoint {
        // A socket somebody named is a daemon somebody chose. Taken as asked for, and left
        // alone: this is the deliberate way out of the arrangement below.
        Endpoint::Local { socket_path: Some(path) } => {
            Ok(Reached { socket_path: path.clone(), tunnel: None, started: false })
        }
        Endpoint::Local { socket_path: None } => {
            let inherited: BTreeMap<String, String> = std::env::vars().collect();
            let home =
                install::muster_home(|name| inherited.get(name).cloned()).ok_or_else(|| {
                    "Muster cannot work out where its daemon's socket goes, because nothing in the \
                 environment says where home is - neither MUSTER_HOME nor HOME. This window will \
                 render nothing. Give the daemon a `socket` in the config file to say outright."
                        .to_string()
                })?;
            let socket = install::socket_path(&home);
            let binary = daemon_binary().ok_or_else(|| {
                "this app was not told where its muster-daemon is, so there is no daemon to start \
                 and this window will render nothing. A build stages it beside the bridge; this \
                 is a bug in how the shell starts Muster."
                    .to_string()
            })?;
            let given = environment::for_daemon(
                &inherited,
                platform_locale().as_deref(),
                commands_path().as_deref(),
            );
            let data = poison::lock(&DAEMON_DATA, "daemon-data").clone();
            let (reached, _) = launch::ensure_running(&launch::Launch {
                binary: binary.as_ref(),
                data: data.as_deref().map(Path::new),
                socket: &socket,
                environment: &given,
            })?;
            let socket_path = socket.display().to_string();
            // Written down only when Muster started it. An adopted daemon belongs to whoever
            // started it, and offering it in Muster's own census would be offering somebody a
            // process to end on Muster's word.
            let started = reached == launch::Reached::Started;
            if started && let Some(directory) = daemon_records_path() {
                records::started(&directory, &socket_path);
            }
            Ok(Reached { socket_path, tunnel: None, started })
        }
        // Somebody's own daemon on another machine: forwarded as asked for, and left alone.
        Endpoint::Ssh { host, options, socket_path: Some(path) } => {
            let tunnel = open_tunnel(daemon, host, options, path.clone())?;
            Ok(Reached {
                socket_path: tunnel.local_socket_path().to_string(),
                tunnel: Some(tunnel),
                started: false,
            })
        }
        // The arrangement the local arm has, one machine further away: whatever is installed
        // for this version over there is started, detached, through the forward.
        Endpoint::Ssh { host, options, socket_path: None } => {
            let far = remote_environment(host, options)?;
            let installed = remote::Installed::on(&far).ok_or_else(|| {
                format!(
                    "{host} answered, and nothing in its environment says where home is, so there \
                     is nowhere to look for its daemon. That machine's panes are absent from the \
                     window and nothing else is affected. Name the daemon's socket in the config \
                     file's `socket` key to say outright."
                )
            })?;
            // Opened before the daemon exists, so everything after this asks "does it answer"
            // through the forwarded path: ssh reaches the far socket per connection, so one that
            // is not there yet costs nothing until something dials it.
            let tunnel =
                open_tunnel(daemon, host, options, installed.socket.display().to_string())?;
            let local = PathBuf::from(tunnel.local_socket_path());
            let (reached, _) = remote::ensure_running(
                &tunnel.remote(),
                &installed,
                &carried(),
                &local,
                &environment::for_far_daemon(&far),
            )?;
            Ok(Reached {
                socket_path: tunnel.local_socket_path().to_string(),
                tunnel: Some(tunnel),
                started: reached == launch::Reached::Started,
            })
        }
    }
}

/// Where a daemon's tunnel puts its ends.
///
/// Named for the daemon rather than numbered, unlike a pane's socket, because there are a
/// handful of these and the name is what makes one recognisable in `lsof` at the moment
/// somebody is wondering which connection is wedged. The pid keeps two Musters apart.
/// One master to a daemon on another machine, forwarding that daemon's socket onto a path here.
fn open_tunnel(
    daemon: &DaemonId,
    host: &str,
    options: &[String],
    remote_socket: String,
) -> Result<Tunnel, String> {
    let reported = daemon.clone();
    Tunnel::open(
        Forward {
            host: host.to_string(),
            options: options.to_vec(),
            control_path: tunnel_path(daemon, "ctl"),
            local_socket: tunnel_path(daemon, "sock"),
            remote_socket,
        },
        // The transport says a host is away and for how long; naming which machine that is in
        // this window, and putting the sentence where somebody sees it, is this side's.
        Arc::new(move |state| tunnel_state(&reported, &state)),
    )
}

/// Turns what a tunnel says about itself into something the person can see.
///
/// A problem rather than a log line, because the run log already carries every drop and every
/// retry and that is where a sequence belongs. What reaches the window is the one thing worth
/// interrupting for: this machine has been away long enough that its panes are lying, and it
/// is not something Muster can fix by trying harder.
///
/// A warning rather than an error. Severity here decides interruption and nothing else, and
/// this is the case the level was written for - Muster is coping, the work on the far machine
/// is untouched, and it may well clear by itself.
fn tunnel_state(daemon: &DaemonId, state: &TunnelState) {
    match state {
        TunnelState::Unreachable { detail } => {
            health(daemon, Health::Stale, detail);
            raise_problem(&reconnect::key(daemon.as_str()), Severity::Warning, detail);
        }
        TunnelState::Reachable => clear_problem(&reconnect::key(daemon.as_str()), "reachable"),
    }
}

fn tunnel_path(daemon: &DaemonId, extension: &str) -> String {
    let name = format!("muster-{}-{daemon}.{extension}", std::process::id());
    std::env::temp_dir().join(name).to_string_lossy().into_owned()
}

/// Everything one attached pane needs to be typed into.
#[derive(Debug)]
pub(crate) struct AttachedPane {
    pub(crate) input: PaneInput,
    /// The socket its bridge reports on. Held because dropping it unlinks the socket and stops
    /// the listener.
    link: Arc<PaneLink>,
}

impl AttachedPane {
    pub(crate) fn link_path(&self) -> &str {
        self.link.socket_path()
    }
}

/// One daemon this process is following.
#[derive(Debug)]
struct Backend {
    mirror: Arc<Mutex<Mirror>>,
    /// The ssh master this daemon is reached through, for a remote one. Held because dropping
    /// it takes the connection down. Absent for a daemon on this machine.
    tunnel: Option<Tunnel>,
    /// Whether Muster started this daemon rather than attaching to one already answering.
    ///
    /// Kept because ending a daemon is a decision somebody has to make about a process holding
    /// work, and "did we start it" is the one thing about it nothing else can reconstruct.
    started: bool,
    /// Where this daemon was actually found, as opposed to how it was asked for: for a remote
    /// one, the local end of its forward, which is what its panes' bridges dial.
    socket_path: String,
    /// How this daemon is asked for changes. One per daemon, because structure belongs to the
    /// daemon rather than to any pane in it.
    channel: Arc<dyn BackendChannel>,
    /// Where its panes' keystrokes go, one connection for all of them.
    input: Arc<dyn InputSink>,
    /// Held because dropping it hangs up and stops following.
    follower: Follower,
}

/// Everything this process holds open, and the composition it holds it for.
#[derive(Debug, Default)]
pub(crate) struct Session {
    composition: Composition,
    backends: BTreeMap<DaemonId, Backend>,
    /// Nested rather than keyed by a pair, so that finding the pane a keystroke is for costs
    /// two lookups and no allocation - a pair key would have to be built, and building one
    /// means cloning both ids on a path that runs per keystroke.
    panes: BTreeMap<DaemonId, BTreeMap<PaneId, Arc<AttachedPane>>>,
    /// Names the next pane's socket. A counter rather than the pane's id: a Unix socket path
    /// has about a hundred bytes to spend and the temporary directory has already spent half
    /// of them, and a backend is free to spell an id with characters a path cannot hold.
    next_socket: u64,
    /// Machines this window has asked for a first tab, once each.
    ///
    /// A window with nothing to show asks a machine for a tab, and a rule with no record of
    /// having asked would ask again on every event any machine sent until the tab arrived.
    ///
    /// **Never emptied.** A machine is asked at most once for the life of this window, which is
    /// what makes closing a machine's last pane leave it empty instead of getting a fresh shell
    /// a moment later (kan a_2I6h18OU6). A daemon that refuses is left in here for the sharper
    /// version of the same reason: a rule that retried a refusal would retry it forever.
    tabs_asked_of: BTreeSet<DaemonId>,

    /// Which window this is, and which window holds each tab.
    ///
    /// A window lists only the tabs it holds (kan a_2Mhi0EZlv). What the composition holds is
    /// kept in line with this, and this is kept in line with the file every window shares.
    holding: Holding,

    /// Which agents have been seen, and so which are `done`.
    ///
    /// Beside the mirrors rather than inside one, because it spans them: a window is focused
    /// or it is not, and that answers for a laptop's panes and a devenv's at once.
    attention: Attention,

    /// Which pane has been told it has the keyboard of this window, for a program that asked
    /// to hear it. Spans the daemons for the reason attention does.
    pane_focus: PaneFocus,

    /// When each pane's agent last changed state, in milliseconds since the epoch.
    ///
    /// Stamped here rather than in the mirror, which is a pure fold over what a daemon said -
    /// and a daemon's events carry no time.
    state_since: BTreeMap<PaneKey, i64>,

    /// The window's own chrome, which spans the daemons for the same reason attention does.
    presentation: Presentation,

    /// How big each pane's text is, for the panes somebody has sized.
    ///
    /// Beside the chrome rather than inside it, because it is not one answer about the window.
    /// It spans the daemons all the same: a window shows a laptop's panes and a devenv's, and
    /// the chord that sizes one has no reason to care which machine it is on.
    font_sizes: FontSizes,

    /// Which panes have had a bridge end, and how recently.
    ///
    /// Beside the sizes and spanning the daemons for the same reason: a laptop pane and a
    /// devenv pane both have bridges, and the rule for replacing one is the same either way -
    /// though it is a devenv the rule was written for, since it is an ssh that dies when a
    /// laptop changes network.
    respawns: Respawns,

    /// Where every pane and tab this window makes gets its name, whichever daemon it is on:
    /// names are unique across machines, so one minter draws them all.
    minter: Arc<Mutex<Minter>>,

    /// The tab a numbered chord has just named, while `numbered_chords = "tab_then_pane"`.
    ///
    /// The whole of the prototype scheme's state, and it is here because on macOS a chord is
    /// a menu item and a menu item's only way to say anything is to dispatch a request - so
    /// this side is the only side that sees both presses. A flag in the shell would be
    /// unreachable from a test, the corpus and the CLI alike.
    ///
    /// Advisory rather than authoritative: [`Session::numbering`] derives what is numbered
    /// from this *and* the roster every time, so a tab that closed while it was armed reads
    /// as disarmed rather than wedging the chords. Always `None` under the settled scheme.
    armed: Option<TabId>,

    /// The last view and roster the shell was sent, so one it already has is not sent again.
    ///
    /// Each costs the shell main-thread work - a forced layout per region, two sidebar diffs,
    /// every badge redrawn - and many republishes change nothing: focusing the pane that
    /// already has the keyboard, a daemon echoing an arrangement the window already holds.
    sent: Sent,
}

#[derive(Debug, Default)]
struct Sent {
    view: Option<ViewChanged>,
    roster: Option<RosterChanged>,
}

impl Sent {
    /// Remembers `view`, and says whether the shell has yet to see it.
    fn view(&mut self, view: &ViewChanged) -> bool {
        if self.view.as_ref() == Some(view) {
            return false;
        }
        self.view = Some(view.clone());
        true
    }

    fn roster(&mut self, roster: &RosterChanged) -> bool {
        if self.roster.as_ref() == Some(roster) {
            return false;
        }
        self.roster = Some(roster.clone());
        true
    }
}

pub(crate) static SESSION: LazyLock<Mutex<Session>> =
    LazyLock::new(|| Mutex::new(Session::default()));

/// Held from settling what the shell is to be sent until it has been sent.
///
/// `Session::sent` remembers the last view and roster so that neither is sent twice, which is
/// only true if they reach the shell in the order they were remembered. Publishes run on the
/// main thread, daemon threads and the watchdog's; without this, one settled first could be
/// sent second, leaving the shell on the older view with nothing left to correct it. Its own
/// lock rather than `SESSION` held longer, because what it covers includes socket writes to
/// bridges. Always taken before `SESSION`, never while holding it.
static PUBLISHING: Mutex<()> = Mutex::new(());

/// Puts this process back where it was before any of it started.
///
/// One window per process is the arrangement everything above is written against, and a global
/// is the honest expression of it - but a test binary is a process too, and one that could not
/// start over was a binary that could hold one test. That is what this is for and the only
/// thing that calls it (`crate::testing`).
///
/// **Every process-wide thing this crate holds belongs here.** The statics above are settings a
/// shell hands over once at startup, and one left behind is the last test's answer arriving in
/// the next one's window - a state path, so a test writes over another's file; bindings, so a
/// chord means what somebody else configured. There is no compiler check for that, so adding a
/// static above means adding a line here, and the way it fails otherwise is a test that passes
/// alone and not in company.
///
/// Dropping the session is the whole teardown: a `Backend` owns its follower, its ssh
/// master and its panes' sockets, and dropping one ends the threads behind all three. So this
/// is the shutdown path that already existed, called deliberately rather than at exit.
pub(crate) fn reset() {
    *poison::lock(&SESSION, "session") = Session::default();

    *poison::lock(&DAEMON_BINARY, "daemon-binary") = None;
    *poison::lock(&PLATFORM_LOCALE, "locale") = None;
    *poison::lock(&COMMANDS, "commands") = None;
    *poison::lock(&STATE, "saved-arrangement") = None;
    *poison::lock(&SHOW, "show") = None;
    *poison::lock(&BINDINGS, "bindings") = None;
    *poison::lock(&PANE_INPUT, "input-settings") = None;
    *poison::lock(&SETTINGS, "daemon-settings") = None;
    *poison::lock(&FEEL, "settings") = None;
    *poison::lock(&CONFIG_PATH, "settings") = None;
    *poison::lock(&APPEARANCE, "settings") = None;
    *poison::lock(&PROBLEMS, "problems") = None;
    *poison::lock(&CONFIGURED_DAEMONS, "settings") = None;
    poison::lock(&DARK, "dark-panes").clear();
    OPENED.store(false, Ordering::Relaxed);

    // Not this file's, and here anyway: what needs resetting is a property of the process
    // rather than of a module, and a caller that had to remember three doors would eventually
    // remember two. The endpoint is given up by asking for nowhere, which is the same path a
    // shell configured with no socket takes.
    watchdog::forget_everything();
    watchdog::unset_deadlines();
    watch::forget_everyone();
    command::listen("");
    ffi::muster_set_event_callback(None);
}

impl Session {
    /// Starts following a daemon, or leaves the one already being followed alone.
    ///
    /// The endpoint and the socket path are both passed because they are different things.
    /// The endpoint is what someone asked for and is what composition writes down; the path
    /// is where this run found it, and is worth nothing to a later one.
    ///
    /// Returns the daemon's connection, for the caller to wait on its first snapshot once it
    /// has let go of the session: the follower applies it on its own thread, and announcing it
    /// takes this lock.
    fn follow(
        &mut self,
        daemon: &Daemon,
        reached: Reached,
    ) -> Result<Option<Arc<muster_daemon_client::follow::Connection>>, String> {
        let id = daemon.id.clone();
        self.composition.attach_daemon(daemon.clone());
        if self.backends.contains_key(&id) {
            return Ok(None);
        }

        let mirror = Arc::new(Mutex::new(Mirror::new()));
        let reporting = id.clone();
        let follower = Follower::start(
            Following {
                socket: reached.socket_path.clone().into(),
                client: format!("muster {}", env!("CARGO_PKG_VERSION")),
                daemon: id.to_string(),
                remote: reached.tunnel.is_some(),
            },
            Arc::clone(&mirror),
            Arc::new(move |notice| announce(&reporting, notice)),
        )
        .map_err(|error| format!("could not start following {id} ({error})"))?;
        if let Some(settings) = poison::lock(&SETTINGS, "daemon-settings").as_ref() {
            follower.configure(settings);
        }
        let connection = follower.connection();
        let description = match &reached.tunnel {
            Some(tunnel) => format!("the daemon {id} on {}", tunnel.host()),
            None => format!("the daemon {id}"),
        };
        let channel = Arc::new(DaemonBackend::new(
            Arc::clone(&connection),
            Arc::clone(&mirror),
            Arc::clone(&self.minter),
            pane_environment(),
            description.clone(),
        ));
        let input =
            Arc::new(DaemonInput::new(Arc::clone(&connection), muster_vt::key_code, description));
        self.backends.insert(
            id,
            Backend {
                mirror,
                tunnel: reached.tunnel,
                started: reached.started,
                socket_path: reached.socket_path,
                channel,
                input,
                follower,
            },
        );
        Ok(Some(connection))
    }

    /// Drops the text sizes somebody set on panes a daemon no longer holds.
    ///
    /// Only where the daemon is answering. A mirror that has gone stale is not evidence a pane
    /// is gone - it is a connection nobody is hearing from - and a pane coming back at the
    /// configured size after a blip would have lost a size set on purpose.
    fn forget_what_closed(&mut self) {
        let gone: Vec<PaneKey> = self
            .font_sizes
            .entries()
            .filter(|(pane, _)| {
                // A daemon nothing is following is not evidence either. An entry for one this
                // window is about to attach again is what makes a size survive a relaunch.
                self.backends.get(&pane.daemon).is_some_and(|backend| {
                    let mirror = poison::lock(&backend.mirror, "mirror");
                    mirror.health() == Health::Connected && mirror.pane(&pane.pane).is_none()
                })
            })
            .map(|(pane, _)| pane.clone())
            .collect();
        self.font_sizes.retain(|pane| !gone.contains(pane));
    }

    /// Whether this pane is in a tab this window holds.
    fn in_a_held_tab(&self, pane: &PaneKey) -> bool {
        self.backends.get(&pane.daemon).is_some_and(|backend| {
            poison::lock(&backend.mirror, "mirror")
                .pane(&pane.pane)
                .is_some_and(|held| self.composition.holds(&held.tab))
        })
    }

    /// Whether the daemon still holds this pane.
    ///
    /// Asked of the mirror, which is a daemon's own answer as of the last thing it said, and
    /// so the only place "did this pane close, or did its connection die" is written down.
    fn holds(&self, pane: &PaneKey) -> bool {
        self.backends.get(&pane.daemon).is_some_and(|backend| {
            poison::lock(&backend.mirror, "mirror").pane(&pane.pane).is_some()
        })
    }

    /// The region a request about a whole tab acts through, or why there is none.
    ///
    /// A tab a closed window holds may still be closed from here, with no region: the daemon does
    /// the closing and the closed window only remembers the tab. A tab an open window holds never
    /// reaches here from a caller, because it is carried to that window first (`forward`).
    fn region_for_tab(
        &self,
        daemon: &DaemonId,
        tab: &TabId,
        closing: bool,
    ) -> Result<Option<RegionId>, Refusal> {
        if let Some(region) = self.composition.region_of(daemon, tab) {
            return Ok(Some(region));
        }
        let described = self
            .backends
            .get(daemon)
            .is_some_and(|backend| poison::lock(&backend.mirror, "mirror").tab(tab).is_some());
        if closing && described && self.holding.elsewhere(tab).is_some() {
            return Ok(None);
        }
        Err(Refusal::Declined(not_showing(daemon)))
    }

    /// Brings composition, and what this process holds open, in line with one daemon.
    fn reconcile(&mut self, daemon: &DaemonId) {
        self.settle_holding(daemon);
        self.prune(daemon);
        self.open_channels(daemon);
    }

    /// Takes the tabs on this machine that nobody holds, if they are this window's to take, and
    /// tells the composition which of the machine's tabs are this window's.
    ///
    /// Before the window has opened this takes nothing when there is a shared record, because
    /// the window has not said it exists and so is never the one a tab joins. Saying so asks again
    /// ([`take_what_nobody_holds`]), and so does coming to the front.
    ///
    /// Says whether it took anything.
    fn settle_holding(&mut self, daemon: &DaemonId) -> bool {
        let described: Vec<TabId> = {
            let Some(backend) = self.backends.get(daemon) else { return false };
            poison::lock(&backend.mirror, "mirror").tabs().map(|tab| tab.id.clone()).collect()
        };
        // Here rather than at each place a daemon is attached or let go, because every one of
        // them reconciles next. Compared first, so the ordinary reconcile writes nothing.
        if !self.holding.follows_exactly(self.backends.keys()) {
            self.holding.follow(self.backends.keys().cloned().collect());
        }
        let taken = self.holding.take_unheld(daemon, &described);
        for tab in described {
            if self.holding.holds(&tab) {
                self.composition.hold(tab);
            }
        }
        !taken.is_empty()
    }

    /// Lets go of what this daemon no longer holds.
    ///
    /// Regions whose tab is gone, and the channels of panes that are gone with them - each one
    /// owns a bound socket and the thread waiting on it, so a window whose panes come and go
    /// all day would otherwise collect both, and neither shows up as anything but a process
    /// that grows.
    fn prune(&mut self, daemon: &DaemonId) {
        let Some(backend) = self.backends.get(daemon) else { return };
        let mirror = poison::lock(&backend.mirror, "mirror");

        self.composition.reconcile(daemon, &mirror);
        let attached = self.panes.entry(daemon.clone()).or_default();
        attached.retain(|pane, _| {
            let held = mirror.pane(pane).is_some();
            if !held {
                // Before the channel is dropped, so that an error about a pane that never
                // became typeable goes with the pane rather than outliving it in the roster,
                // naming something nobody can look at any more.
                let key = PaneKey::new(daemon, pane);
                watchdog::closed(&key);
                // And nothing is owed about its bridge either. A window whose panes come and
                // go all day would otherwise accumulate one entry per pane it ever held.
                poison::lock(&DARK, "dark-panes").remove(&key);
            }
            held
        });
        // What was tried for a pane goes with the pane. A pane closed from another client
        // never reports a bridge exiting, so without this the map keeps a row for every pane
        // the window has ever held.
        self.respawns.retain(|pane| &pane.daemon != daemon || mirror.pane(&pane.pane).is_some());
    }

    /// Opens a channel for every pane this daemon has on screen and does not already have one.
    ///
    /// A channel per pane in every region this daemon shows, not only the one the keyboard is
    /// on: the view names a socket for each leaf and a shell renders a surface per leaf, so a
    /// pane the daemon added is one Muster has to be ready to be typed into before anyone
    /// looks at it.
    ///
    /// On screen means the tree the view will publish, narrowed through the same
    /// [`zoom_filling`] the view narrows with, and not every leaf the tab's tree names. A
    /// zoomed region draws one pane, so binding a socket for each of the others leaves sockets
    /// nothing will ever dial - and five seconds later the typeable watch reports each of them
    /// as a pane swallowing keystrokes. They were fine; nothing was drawing them. An alarm on
    /// a healthy window is what teaches somebody to ignore the alarm that matters, and this
    /// one fired on every launch onto a zoomed tab.
    ///
    /// Nothing is closed here for a pane that stops showing. A hidden pane's surface is parked
    /// rather than released, so its bridge is still dialed into the socket it has, and taking
    /// that socket away would be taking the keyboard from a pane one keystroke can bring back.
    ///
    /// Called from `publish`, which is what makes it a rule rather than a step somebody has to
    /// remember. Every path that changes what is on screen ends in a publish, and a view naming
    /// a pane with no socket is a pane a shell must not build a surface for - so it renders
    /// blank until something else republishes, which in one shipped case was nothing at all.
    fn open_channels(&mut self, daemon: &DaemonId) {
        // In two passes, because opening a channel needs the whole session and reading the
        // mirror borrows one daemon out of it. Nothing can change in between: the caller
        // holds the session across both.
        let wanted: Vec<PaneId> = {
            let Some(backend) = self.backends.get(daemon) else { return };
            let mirror = poison::lock(&backend.mirror, "mirror");
            let attached = self.panes.entry(daemon.clone()).or_default();

            let showing = self.composition.showing().cloned();
            self.composition
                .regions()
                .filter(|region| &region.daemon == daemon)
                .filter_map(|region| Some((region, mirror.tab(showing.as_ref()?)?)))
                .flat_map(|(region, layout)| match zoom_filling(region, Some(layout)) {
                    Some(filling) => vec![filling],
                    None => layout.root.panes().into_iter().cloned().collect(),
                })
                .filter(|pane| !attached.contains_key(pane) && mirror.pane(pane).is_some())
                .collect()
        };

        for pane in wanted {
            if let Err(refusal) = self.open_channel(daemon, &pane) {
                // Logged rather than returned: nothing called this to attach that pane, and
                // the pane still renders. What it costs is that one pane's keyboard, so the
                // line has to name it.
                log::error(
                    "pane.channel.unavailable",
                    fields! {
                        "daemon" => daemon.to_string(),
                        "pane" => pane.to_string(),
                        "detail" => refusal.clone(),
                        "impact" => "this pane renders and ignores the keyboard; every other \
                                     pane in the window is unaffected",
                    },
                );
            }
        }
    }

    /// Binds a pane's socket and builds its input path.
    ///
    /// The socket is bound before this returns, and so before the shell is told about the
    /// pane it belongs to - which is what stops a bridge losing a race against its own
    /// listener.
    fn open_channel(&mut self, daemon: &DaemonId, pane: &PaneId) -> Result<(), String> {
        if self.panes.get(daemon).is_some_and(|held| held.contains_key(pane)) {
            return Ok(());
        }
        let backend = self.backends.get(daemon).ok_or_else(|| {
            format!(
                "the daemon {daemon} is not being followed, so there is nowhere to send \
                 this pane's input. This is a bug in the core rather than a state to \
                 recover from: a channel is only ever opened for a daemon already \
                 attached."
            )
        })?;
        let sink = Arc::clone(&backend.input);
        let path = self.next_socket_path();
        let attached = PaneKey::new(daemon, pane);
        let stopped = attached.clone();
        let painting = attached.clone();
        let asked = attached.clone();
        let link = PaneLink::bind(
            path,
            Reports {
                attached: Box::new(move || typeable(&attached.daemon, &attached.pane)),
                exited: Box::new(move |ended| bridge_ended(&stopped, &ended)),
                painted: Box::new(move || watchdog::painted(&painting)),
            },
        )
        .map_err(|error| {
            format!(
                "could not bind the socket this pane's bridge reports on ({error}). Usual \
                 causes: a full or read-only temporary directory."
            )
        })?;

        self.panes.entry(daemon.clone()).or_default().insert(
            pane.clone(),
            Arc::new(AttachedPane {
                input: PaneInput::new(pane.clone(), sink, &pane_input())
                    // Hung on the input path rather than on its call sites: something reached
                    // this pane, so output is owed and the window can tell a pane that stopped
                    // painting from one whose agent has nothing to say (kan a_2LMRCug0P).
                    .delivering_to(Arc::new(move || watchdog::typed(&asked))),
                link: Arc::new(link),
            }),
        );
        // The socket is bound and the shell has not been told about it yet, so this is the
        // earliest moment the wait for a bridge can be said to have started.
        watchdog::opened(PaneKey::new(daemon, pane));
        Ok(())
    }

    fn channel(&self, daemon: &DaemonId, pane: &PaneId) -> Option<&Arc<AttachedPane>> {
        self.panes.get(daemon)?.get(pane)
    }

    /// What one daemon's mirror says a pane's agent is doing.
    ///
    /// Scoped to the daemon the key names rather than searched, like every other lookup here.
    fn agent_state(&self, pane: &PaneKey) -> Option<AgentState> {
        let mirror = poison::lock(&self.backends.get(&pane.daemon)?.mirror, "mirror");
        mirror.agent_state(&pane.pane)
    }

    /// The state this window paints for a pane before `done` is laid over it: `waiting` for an
    /// idle agent waiting on its own work.
    fn presented_state(&self, pane: &PaneKey) -> Option<AgentState> {
        let mirror = poison::lock(&self.backends.get(&pane.daemon)?.mirror, "mirror");
        mirror.pane(&pane.pane).map(muster_core::mirror::Pane::presented_state)
    }

    /// Whether a pane's daemon says its agent finished and nobody has seen it since.
    fn finished_unseen(&self, pane: &PaneKey) -> bool {
        self.backends.get(&pane.daemon).is_some_and(|backend| {
            let mirror = poison::lock(&backend.mirror, "mirror");
            mirror.pane(&pane.pane).is_some_and(|record| record.finished_unseen)
        })
    }

    /// The attached panes to tell they gained or lost focus, held so that telling them can wait
    /// until this lock is let go. A pane no longer attached has nobody left to tell.
    fn focus_reports(&self, told: Vec<(PaneKey, bool)>) -> Vec<(Arc<AttachedPane>, bool)> {
        told.into_iter()
            .filter_map(|(pane, focused)| {
                Some((Arc::clone(self.panes.get(&pane.daemon)?.get(&pane.pane)?), focused))
            })
            .collect()
    }

    /// Hands attention a pane's record as its daemon now has it, and tells the daemon when the
    /// pane finished in front of somebody.
    fn observe(&mut self, pane: &PaneKey) -> Option<Attend> {
        let state = self.agent_state(pane)?;
        let observed = self.attention.observed(pane, state, self.finished_unseen(pane));
        if observed.reported {
            self.report_seen(std::slice::from_ref(pane));
        }
        observed.attend
    }

    /// Tells each daemon which of its finished panes this window has just shown somebody, so
    /// that it clears them for every window (`muster_core::attention`).
    ///
    /// Queued to each connection's writer rather than waited on, so it is safe under the
    /// session's lock. One that finds its daemon disconnected is not held: the window reports
    /// what it is showing again when the daemon comes back.
    fn report_seen(&self, panes: &[PaneKey]) {
        let mut by_daemon: BTreeMap<&DaemonId, Vec<PaneId>> = BTreeMap::new();
        for pane in panes {
            by_daemon.entry(&pane.daemon).or_default().push(pane.pane.clone());
        }
        for (daemon, panes) in by_daemon {
            let keys: Vec<PaneKey> = panes.iter().map(|pane| PaneKey::new(daemon, pane)).collect();
            let sent = self
                .backends
                .get(daemon)
                .is_some_and(|backend| backend.follower.seen(&panes, move || seen_refused(&keys)));
            log::info(
                "attention.seen",
                fields! {
                    "daemon" => daemon.to_string(),
                    "panes" => panes.iter().map(PaneId::as_str).collect::<Vec<&str>>().join(","),
                    "sent" => sent,
                },
            );
        }
    }

    /// One pane's agent as this window paints it, from what its daemon said, `waiting` included.
    fn presented(&self, pane: &PaneKey, state: AgentState) -> PaneAgent {
        PaneAgent {
            pane: pane.clone(),
            state: self.attention.presented(pane, state),
            since_ms: self.state_since.get(pane).copied().unwrap_or_default(),
        }
    }

    /// Every pane every followed daemon holds, as this window paints it, daemon by daemon.
    fn agents(&self) -> Vec<PaneAgent> {
        let mut agents = Vec::new();
        for (id, backend) in &self.backends {
            let mirror = poison::lock(&backend.mirror, "mirror");
            for pane in mirror.panes() {
                agents.push(self.presented(&PaneKey::new(id, &pane.id), pane.presented_state()));
            }
        }
        agents
    }

    /// Puts a region onto the tab holding this pane, so that something can show it.
    ///
    /// The mirror is what knows which tab a pane is in, so the lookup is here and the policy
    /// - which region, or a new one - is in the composition record where the rest of it is.
    ///
    /// Refuses by name rather than silently doing nothing. A pane the daemon has never heard
    /// of and a pane that closed while a click was in flight look identical from a sidebar
    /// row, and both leave the keyboard where it was.
    /// The refusal carries its kind, and one of the three is `NotThere` on purpose: a mirror
    /// with no such pane is the same fact a daemon would answer with, reached without a round
    /// trip. Saying so is what lets the caller reword it - a request naming a pane no machine
    /// holds is answered about no machine (`handler::placed`), rather than blaming whichever
    /// one happened to have the keyboard.
    fn surface(&mut self, daemon: &DaemonId, pane: &PaneId) -> Result<RegionId, Refusal> {
        let tab = {
            let backend = self.backends.get(daemon).ok_or_else(|| {
                Refusal::Declined(format!(
                    "this window is not following a daemon called {daemon}, so there is \
                     nothing to show {pane} in and the keyboard stayed where it was."
                ))
            })?;
            let mirror = poison::lock(&backend.mirror, "mirror");
            let held = mirror.pane(pane).ok_or_else(|| {
                Refusal::NotThere(format!(
                    "{daemon} holds no pane called {pane}, so the keyboard stayed where it \
                     was. Most likely it closed while this was in flight, which an entry in a \
                     list outlives by a moment."
                ))
            })?;
            held.tab.clone()
        };
        // Another window's tab stays in that window. Surfacing it here would take its terminals
        // from the window showing them, which is the failure kan a_2Mhi0EZlv was raised for.
        if !self.holding.holds(&tab) {
            if let Some(window) = self.holding.elsewhere(&tab) {
                return Err(taken_elsewhere(pane, &tab, &window.name));
            }
            // Kept under the hold, which reads the record afresh: this window's copy may be a
            // moment behind another window taking the tab.
            self.holding.keep(std::slice::from_ref(&tab));
            if let Some(window) = self.holding.elsewhere(&tab) {
                return Err(taken_elsewhere(pane, &tab, &window.name));
            }
        }
        self.composition.surface(daemon, &tab).ok_or_else(|| {
            Refusal::Declined(format!(
                "{daemon} is followed but not attached to this window's composition, so no \
                 region could be opened onto {pane}."
            ))
        })
    }

    /// What this window is showing, right now.
    ///
    /// Every daemon's mirror is locked for the length of it, in the map's own order. That
    /// order is what makes it safe: the only other path taking two of these locks takes them
    /// one at a time and lets go of the mirror before it asks for the session.
    fn view(&self) -> View {
        let mirrors: BTreeMap<&DaemonId, MutexGuard<'_, Mirror>> = self
            .backends
            .iter()
            .map(|(id, backend)| (id, poison::lock(&backend.mirror, "mirror")))
            .collect();
        View::of(
            &self.composition,
            |daemon| mirrors.get(daemon).map(|held| &**held),
            |daemon| Some(self.backends.get(daemon)?.socket_path.clone()),
            |daemon| self.backends.get(daemon).is_some_and(|backend| backend.tunnel.is_some()),
            |daemon, pane| {
                let key = PaneKey::new(daemon, pane);
                ViewPane {
                    id: pane.clone(),
                    link_socket_path: self
                        .channel(daemon, pane)
                        .map(|held| held.link_path().to_string()),
                    font_size_offset: self.font_sizes.offset(&key),
                    bridge_restarts: self.respawns.restarts(&key),
                }
            },
        )
    }

    /// Everything the attached daemons hold, and which of it is on screen.
    ///
    /// Takes the view rather than recomputing what is visible, so the two answers cannot
    /// disagree - a row marked hidden while its surface is on screen is the sidebar being
    /// wrong about the window beside it.
    ///
    /// Locks every mirror for the length of it, in the map's own order, on the same terms as
    /// [`Session::view`].
    fn roster(&self, view: &View) -> Roster {
        let mirrors: BTreeMap<&DaemonId, MutexGuard<'_, Mirror>> = self
            .backends
            .iter()
            .map(|(id, backend)| (id, poison::lock(&backend.mirror, "mirror")))
            .collect();
        Roster::of(
            &self.composition,
            |daemon| mirrors.get(daemon).map(|held| &**held),
            view.showing(),
        )
    }

    /// Every other window the record knows, with a roster of the tabs each holds.
    ///
    /// Built through a composition of its own, holding that window's tabs, so a tab is described
    /// the same way here as in the window that has it: the same labels, the same pane order.
    fn other_windows(&self) -> Vec<(HeldWindow, Roster)> {
        let mirrors: BTreeMap<&DaemonId, MutexGuard<'_, Mirror>> = self
            .backends
            .iter()
            .map(|(id, backend)| (id, poison::lock(&backend.mirror, "mirror")))
            .collect();
        let holders = self.holding.holders();
        holders
            .windows()
            .filter(|window| &window.name != self.holding.me())
            .map(|window| {
                let mut theirs = Composition::new();
                for daemon in self.composition.daemons() {
                    theirs.attach_daemon(daemon.clone());
                }
                for tab in holders.held_by(&window.name) {
                    theirs.hold(tab.clone());
                }
                for (daemon, mirror) in &mirrors {
                    theirs.reconcile(daemon, mirror);
                }
                let roster = Roster::of(
                    &theirs,
                    |daemon| mirrors.get(daemon).map(|held| &**held),
                    &BTreeSet::new(),
                );
                (window.clone(), roster)
            })
            .collect()
    }

    /// What ⌘1 to ⌘9 name at this moment.
    ///
    /// The scheme is the config file's answer and the armed tab is this session's; putting
    /// the two together is [`Numbering::of`]'s, so that the corpus is exercising the same
    /// function the window runs on rather than a second copy of the same reasoning.
    fn numbering(&self, roster: &Roster) -> Numbering {
        Numbering::of(feel().numbered_chords, self.armed.as_ref(), roster)
    }

    /// The pane this window's keyboard feeds.
    ///
    /// Handed back behind an `Arc` so the caller can let go of this lock before it sends
    /// anything. A send can be a round trip to a daemon, and holding the session across one
    /// would stall every event arriving from every other daemon behind a wedged one.
    fn keyboard_pane(&self) -> Option<Arc<AttachedPane>> {
        let region = self.composition.focused_region()?;
        self.panes.get(&region.daemon)?.get(region.pane.as_ref()?).map(Arc::clone)
    }

    /// Which region would draw this pane, whether or not its tab is the one on screen.
    ///
    /// Scoped to the daemon the key names rather than searched across all of them, like every
    /// other lookup here.
    ///
    /// A pane in a tab the window is not showing still has one, and that is the whole of what
    /// this answers now that a window shows one tab at a time: every pane but the handful on
    /// screen is in a background tab, and a rule that refused them would refuse going to a pane
    /// that finished unseen - which is the feature. `None` means the window holds no tab with
    /// this pane in it at all, which is a pane in a session it is not attached to.
    fn region_holding(&self, daemon: &DaemonId, pane: &PaneId) -> Option<RegionId> {
        let backend = self.backends.get(daemon)?;
        let held = poison::lock(&backend.mirror, "mirror");
        let tab = held.pane(pane)?.tab.clone();
        self.composition.region_of(daemon, &tab)
    }

    fn channel_of(&self, daemon: &DaemonId) -> Option<Arc<dyn BackendChannel>> {
        self.backends.get(daemon).map(|backend| Arc::clone(&backend.channel))
    }

    fn next_socket_path(&mut self) -> String {
        // A pid in the name because nothing else can legitimately own this path, which is
        // what makes unlinking a stale one safe.
        let name = format!("muster-{}-{}.sock", std::process::id(), self.next_socket);
        self.next_socket += 1;
        std::env::temp_dir().join(name).to_string_lossy().into_owned()
    }
}

/// The pane this window's keyboard feeds, if it has one.
pub(crate) fn keyboard_pane() -> Option<Arc<AttachedPane>> {
    poison::lock(&SESSION, "session").keyboard_pane()
}

/// A pane this window is drawing, by name: the one under the pointer, or the one a held paste
/// came from.
pub(crate) fn attached_pane(daemon: &DaemonId, pane: &PaneId) -> Option<Arc<AttachedPane>> {
    poison::lock(&SESSION, "session").panes.get(daemon)?.get(pane).map(Arc::clone)
}

/// Whether the config lets a program in a pane set the clipboard.
fn clipboard_writes_allowed() -> bool {
    poison::lock(&SETTINGS, "daemon-settings")
        .as_ref()
        .is_none_or(|settings| settings.clipboard_write.allowed())
}

/// Whether this window's keyboard follows a pane the request makes.
///
/// Only ever consulted for a request that makes one, and it is the one thing about a mutation
/// that is Muster's own answer rather than the daemon's. Two callers want opposite things:
/// pressing a key means "I made this and I am looking at it", and a script means "make it and
/// leave my cursor alone" - an agent opening three panes must not drag somebody's keyboard
/// through all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keyboard {
    Follows,
    StaysPut,
}

/// A submission's refusal and its unanswered detail, as two log fields of which at most one is
/// set - so a search for refusals in a run log does not turn up changes that may have happened.
fn refused_or_unanswered<T>(outcome: &Result<T, Refusal>) -> (String, String) {
    match outcome {
        Ok(_) => (String::new(), String::new()),
        Err(Refusal::Unanswered(detail)) => (String::new(), detail.clone()),
        Err(refusal) => (refusal.to_string(), String::new()),
    }
}

/// Asks the daemon showing this window for a change.
///
/// Nothing is applied here that the daemon did not say: what happened arrives on the daemon's
/// own events, which reach the mirror before the answer does, so by the time this returns the
/// mirror already shows it (`architecture.md`, view = f(daemon state)).
///
/// The one exception is where the keyboard ends up, which is Muster's own state and not the
/// daemon's. A split you asked for takes it, because that is what pressing the key meant.
///
/// The channel is taken out from under the lock before the request goes, because a request
/// is a round trip and holding the session across one would stall every event arriving from
/// every other daemon behind a wedged one.
pub(crate) fn submit(
    daemon: &DaemonId,
    intent: &BackendIntent,
    keyboard: Keyboard,
) -> Result<Option<PaneId>, Refusal> {
    let (region, source, channel) = {
        let mut session = poison::lock(&SESSION, "session");
        // Which pane this request came from, for a pane it may be about to make. A split names
        // the pane it splits; ⌘T names nothing, so the answer is the pane in front of whoever
        // asked. Read here rather than after the round trip, because by then the keyboard may
        // have moved.
        let source = match intent {
            BackendIntent::SplitPane { pane, .. } => Some(PaneKey::new(daemon, pane)),
            BackendIntent::CreateTab { .. } => session
                .composition
                .focused_region()
                .and_then(|region| Some(PaneKey::new(&region.daemon, region.pane.as_ref()?))),
            _ => None,
        };
        // Which region this is about, for the keyboard afterwards. None is an answer rather
        // than a failure for an intent that names nothing on screen.
        let region = match intent {
            // A rename is about a thing rather than about what is on screen, and requiring a
            // region would refuse the case the feature exists for: the sidebar lists every
            // pane every daemon holds, and the ones worth naming are the ones no region is
            // showing. Arranging the list is the same: the rows worth dragging are very often
            // the ones no region is showing. A new tab has no region until it exists.
            BackendIntent::CreateTab { .. }
            | BackendIntent::RenamePane { .. }
            | BackendIntent::RenameTab { .. }
            | BackendIntent::SwapPanes { .. }
            | BackendIntent::MovePane { .. } => None,
            // Where a new pane goes is a fact about the tab's tree, on the same terms as
            // moving one, so this asks the window rather than requiring it: the keyboard
            // follows the new pane when a region is showing the tab it landed in, and stays
            // where it is when none is. Requiring a region refused the case the feature is
            // most needed for - an agent told to make panes, whose own pane is in a tab
            // somebody moved off screen - and the way through was taking the keyboard off
            // whatever a person was doing in the tab that is on screen.
            //
            // Closing deliberately stays below. Its argument is the same on paper and its
            // risk is not: it destroys something, and `muster docs limits` already singles
            // it out as the one command whose default destroys the pane it runs in.
            BackendIntent::SplitPane { pane, .. } => session.region_holding(daemon, pane),
            BackendIntent::ClosePane { pane }
            | BackendIntent::ResizePane { pane, .. }
            | BackendIntent::ZoomPane { pane } => Some(
                session
                    .region_holding(daemon, pane)
                    .ok_or_else(|| Refusal::Declined(not_showing(daemon)))?,
            ),
            // Both keep the region requirement, which now means the window holds this tab
            // rather than that it is the tab on screen. A window shows one tab at a time, so
            // the old reading would refuse closing any tab but the one you are looking at -
            // and `muster tab close --tab <t>` naming another is the ordinary case. What it
            // still refuses is a tab in a session this window is not attached to, which is
            // what the guard was protecting against.
            BackendIntent::CloseTab { tab } | BackendIntent::SetSplitRatio { tab, .. } => session
                .region_for_tab(
                daemon,
                tab,
                matches!(intent, BackendIntent::CloseTab { .. }),
            )?,
        };
        let channel = session.channel_of(daemon).ok_or_else(|| {
            Refusal::Declined(format!(
                "the daemon {daemon} is in this window's composition and is not being \
                     followed, which is a bug in the core rather than a state to recover from"
            ))
        })?;
        // Taken before asking. The daemon describes a new tab to every window before this one
        // hears the answer, and a tab nobody held in that moment would go to whichever window
        // came to the front last (kan a_2Mhi0EZlv). One the request then fails to make is a
        // row for a name nothing will ever use, which the next window to open prunes.
        if let Some(tab) = made_tab(intent) {
            session.holding.making(tab);
        }
        (region, source, channel)
    };

    let outcome = channel.submit(intent);
    let (refused, unanswered) = refused_or_unanswered(&outcome);
    log::info(
        "intent.submitted",
        fields! {
            "intent" => intent.redacted(),
            "backend" => channel.description(),
            "created" => outcome.as_ref().ok().and_then(|outcome| outcome.created.clone())
                .map(|pane| pane.to_string()).unwrap_or_default(),
            "refused" => refused,
            "unanswered" => unanswered,
        },
    );
    let Ok(Outcome { created, created_tab }) = &outcome else {
        return outcome.map(|_| None);
    };

    let mut session = poison::lock(&SESSION, "session");
    // A tab this request made is shown. Not for a move, which is the one request here that
    // makes a tab without being about one: it makes a place to put a pane. Bringing that tab on
    // screen would put the tab somebody was working in behind it, so "pull that pane out of the
    // split" would answer by moving them somewhere they did not ask to go - and an agent
    // pulling another agent's pane out would lose its own place doing it. The tab is listed and
    // named, and `muster tab focus` is how anybody who does want to go there says so.
    if !matches!(intent, BackendIntent::MovePane { .. })
        && let Some(tab) = created_tab
    {
        session.composition.hold(tab.clone());
        session.composition.surface(daemon, tab);
    }
    if let Some(created) = created {
        let made = PaneKey::new(daemon, created);
        // A pane opens at the size of the pane it came from, which is what Ghostty does
        // (`window-inherit-font-size`, on by default) and what somebody who has finally made a
        // pane readable means by splitting it. Whatever the keyboard does: an agent's `muster
        // pane new` leaves the cursor alone and still makes a pane beside one that was sized.
        //
        // A pane another client made inherits nothing. There is no request to have come from,
        // and taking whatever this window happened to be focused on would be an answer nobody
        // asked for.
        if let Some(source) = &source {
            session.font_sizes.inherit(&made, source);
        }
        if let (Some(region), Keyboard::Follows) = (region, keyboard) {
            session.composition.focus_pane(region, created.clone());
        }
    }
    drop(session);
    // Always, so a caller that asks for the view next sees what it asked for. The mirror is
    // already current, but the daemon's notices are announced on a thread of their own, and
    // the publish behind them may not have run yet. One that finds nothing new sends nothing.
    publish("intent");
    // The pane, so a caller can name it in its next breath: the name was minted inside this
    // call. The refusal's kind survives rather than being flattened to its sentence. A daemon
    // saying it does not hold what was named is the one a caller may have to reword: asked
    // about a pane no machine holds, its answer is correct on its own terms and misleading
    // where it lands (`handler::placed`).
    Ok(created.clone())
}

/// The tab a request makes under a name the window chose, when it makes one.
///
/// A move into a Muster tab on another machine makes a part of that tab here, under a name the
/// window already holds, so it is not one.
fn made_tab(intent: &BackendIntent) -> Option<&TabId> {
    match intent {
        BackendIntent::CreateTab { tab, .. }
        | BackendIntent::MovePane { to: MoveDestination::NewTab { tab, .. }, .. } => Some(tab),
        _ => None,
    }
}

/// Takes the shell's word that nothing is painting a pane, and starts another bridge if the
/// pane is still there to paint.
///
/// The shell knows one thing the core cannot see - its own subprocess ended - and the core
/// knows the one place to look it up. A pane the daemon has dropped disappears from the window
/// here. A pane it still holds gets a replacement, which is what a laptop swapping ethernet for
/// wifi needs: the ssh under every devenv pane dies with the route, and until this the panes
/// stayed on screen showing a dead terminal until somebody relaunched Muster.
///
/// `process_alive` is what separates the two things a surface ending can mean. False is the
/// bridge exiting on its own, which is the case worth replacing. True is Muster tearing the
/// surface down - the pane left the window, or its surface is being rebuilt - and starting
/// another bridge for that would be racing the one that is about to start.
///
/// It does *not* separate "the connection blinked" from "the pane is gone", which is what the
/// card that asked for this expected of it: a bridge whose pane closed exits on its own too.
/// The mirror answers that, and the interval between exits answers the harder question of
/// whether replacing it is going to help (`muster_core::respawn`).
pub(crate) fn bridge_exited(daemon: &str, pane: &str, process_alive: bool) {
    let daemon = DaemonId::new(if daemon.is_empty() { LOCAL } else { daemon });
    log::info(
        "bridge.exited.reported",
        fields! {
            "daemon" => daemon.to_string(),
            "pane" => pane.to_string(),
            "process_alive" => process_alive.to_string(),
        },
    );
    let key = PaneKey::new(&daemon, &PaneId::new(pane));
    if process_alive {
        // The wait starts again. A pane keeps its link while its surface is thrown away and
        // built again, so the replacement bridge has to attach too - and a replacement that
        // never arrives is the same dark pane, which is the case `bridge_link` names as the
        // reason its accept loop runs more than once.
        watchdog::opened(key);
        return;
    }
    // Nothing to add about how it ended: this arrival says only that a surface's command is
    // gone, which is `Ended::unsaid` by definition.
    bridge_ended(&key, &Ended::unsaid());
}

/// A pane's bridge has stopped, said in its own words on the socket the app bound for it.
///
/// The arrival that actually happens. `bridge_exited` above is the renderer's, which two field
/// runs on 0.4.1 show never coming (kan a_2IRcMjFs0); this one needs no cooperation from
/// libghostty or from the dying process, because what ended is a connection this window owns.
pub(crate) fn bridge_ended(pane: &PaneKey, ended: &Ended) {
    // One death, however many things noticed it. Both watches can fire for one bridge, and
    // counting the second would spend a pane's replacements twice as fast as it earned them.
    //
    // Scoped rather than left to the temporary's lifetime, because everything below this takes
    // `SESSION` and a guard living to the end of the `if` statement would make this the one
    // place that holds `DARK` across a call - which is the whole of what the leaf-lock rule on
    // it is for.
    let news = { poison::lock(&DARK, "dark-panes").insert(pane.clone()) };
    if !news {
        return;
    }
    log::info(
        "bridge.ended",
        fields! {
            "pane" => pane.to_string(),
            "ending" => ended.ending.as_str(),
            "reason" => ended.reason.clone().unwrap_or_else(|| "(it said nothing)".into()),
            "rendered" => ended.rendered.to_string(),
        },
    );
    // The wait starts again, carrying what this bridge said: a pane that stays dark after a
    // refused attach can then say why rather than pointing at a log file.
    watchdog::ended(pane.clone(), ended.clone());
    // The mirror may not have heard yet that a pane whose bridge was let go has closed: the
    // bridge hears on its stream and the window on its control connection, in no order. A
    // bridge whose pane closed says `gone`, which is never replaced, so nothing here waits.
    replace_bridge(pane, ended.ending);
}

/// Starts another bridge for a pane whose last one ended, or says why it will not.
///
/// Nothing is spawned here. The shell owns the surfaces and a bridge is a surface's command,
/// so what this does is count the replacement and publish - and the view carrying a number the
/// shell has not seen for this pane is what makes it build one.
fn replace_bridge(pane: &PaneKey, ending: Ending) {
    let (decision, why) = {
        let mut session = poison::lock(&SESSION, "session");
        let gone = if !session.holds(pane) {
            Some("the daemon no longer holds this pane")
        } else if !session.in_a_held_tab(pane) {
            // Its tab went to another window, and letting go of the pane is what this bridge
            // ending was. Another would take the pane back from the window that has the tab
            // now.
            Some("its tab is in another window now")
        } else {
            None
        };
        match gone {
            None => (Some(session.respawns.ended(pane, clock::monotonic_now(), ending)), ""),
            Some(why) => {
                // The pane closed or left, which are the other reasons a bridge exits on its
                // own. What is left is the record of what was tried, which belongs to a pane
                // this window no longer draws.
                session.respawns.forget(pane);
                // And the wait `bridge_ended` has just started for it, with its dark-pane entry.
                // `prune` takes both back for a pane the mirror drops, but when the mirror
                // dropped this one before its bridge's ending arrived, that prune has already
                // run and will not run for this pane again - so nothing else would ever remove
                // them. Both locks are leaves, so taking them under `SESSION` is allowed.
                watchdog::closed(pane);
                poison::lock(&DARK, "dark-panes").remove(pane);
                (None, why)
            }
        }
    };

    let Some(decision) = decision else {
        log::info("bridge.replacing.skipped", fields! { "pane" => pane.to_string(), "why" => why });
        return;
    };
    match decision {
        Decision::Start(count) => {
            log::info(
                "bridge.replacing",
                fields! { "pane" => pane.to_string(), "attempt" => count.to_string() },
            );
            publish("bridge_replaced");
        }
        // Written down and not raised in the roster, although this is a pane nobody can type
        // into. The typeable watch already reports exactly that, and reports it here: the wait
        // restarted when the bridge exited, so five seconds after this a pane with nothing
        // dialing its socket says so on its own row. A second problem beside it would be two
        // rows about one pane, and this is the half that belongs in the log - what was tried,
        // and the one remedy nobody guesses.
        Decision::GiveUp(tried) => log::warn(
            "bridge.replacing.stopped",
            fields! {
                "pane" => pane.to_string(),
                "tried" => tried.to_string(),
                "detail" => respawn::gave_up(pane, tried),
            },
        ),
        // Not a warning. Everything worked: somebody asked for this pane in another window and
        // got it, which is the arrangement the daemon allows and the one Muster asked for on
        // their behalf. What would be wrong is taking it back, and this is that not happening.
        Decision::Yield => log::info(
            "bridge.yielded",
            fields! { "pane" => pane.to_string(), "detail" => respawn::yielded(pane) },
        ),
        // Nothing to attach to. A replacement here would be a bridge aimed at a pane the
        // daemon has just said does not exist, and its failure to dial would read as a machine
        // nobody can reach (kan a_2LMpvavhA).
        Decision::Leave => log::info(
            "bridge.replacing.skipped",
            fields! {
                "pane" => pane.to_string(),
                "why" => "the daemon says this pane's terminal no longer exists",
            },
        ),
    }
}

/// Gives a pane a bridge because somebody asked for one, and says whether there was a pane.
///
/// A person asking is not a retry, so the run of failures starts over rather than continuing.
/// Whoever ran this has usually just done something about the cause - brought the machine
/// back, or the network to it - and it is also the only way back for a pane the limit has
/// stopped rebuilding, which is what makes stopping affordable at all.
///
/// `false` is a pane no mirror in this window holds. Not an error here: the caller phrases it,
/// because the same answer reaches a person typing a name and a menu item that read one off
/// the view it was drawn from.
pub(crate) fn reattach(pane: &PaneKey) -> bool {
    let restarts = {
        let mut session = poison::lock(&SESSION, "session");
        if !session.holds(pane) {
            return false;
        }
        session.respawns.asked(pane, clock::monotonic_now())
    };
    log::info(
        "bridge.reattach.asked",
        fields! { "pane" => pane.to_string(), "restarts" => restarts.to_string() },
    );
    publish("reattach");
    true
}

/// Asks for a bridge for a pane nothing has dialed, because nothing else is going to.
///
/// The other door into [`replace_bridge`]'s one mechanism, and the reason it exists: that one
/// runs when a bridge *ends*, and a replacement that was decided on and never started has no
/// exit to run it. Everything downstream is identical - count it and publish, and the view
/// carrying a number the shell has not seen is what makes it build the surface a bridge is the
/// command of.
///
/// Called from the watch that already knows which panes nothing is dialing, so this is told
/// when rather than deciding it. Whether asking is right at all is `respawn`'s.
pub(crate) fn bridge_stalled(pane: &PaneKey, deadline: u64) {
    let asked = {
        let mut session = poison::lock(&SESSION, "session");
        if !session.holds(pane) {
            session.respawns.forget(pane);
            return;
        }
        session.respawns.stalled(pane, clock::monotonic_now())
    };

    // `None` is a pane whose bridges are ending on their own, so the replacement policy has
    // already answered for it - it started another, or it stopped, or it left the pane to the
    // window that took it. Asking here would restart a ladder that has just stopped.
    let Some(restarts) = asked else { return };
    log::info(
        "bridge.stalled",
        fields! {
            "pane" => pane.to_string(),
            "restarts" => restarts.to_string(),
            "quiet_for_ms" => (deadline / 1_000_000).to_string(),
        },
    );
    publish("bridge_stalled");
}

/// Why an intent about something on screen could not be sent.
fn not_showing(daemon: &DaemonId) -> String {
    format!(
        "the daemon {daemon} is not showing that pane or tab in this window, so nothing was \
         asked of anything. Either it closed while this was in flight, or the request names \
         something in a session this window is not attached to."
    )
}

/// Points this window's keyboard at a pane.
///
/// No daemon is told. Which pane has the keyboard is Muster's own cursor, and a daemon serving
/// several windows has no single answer to hold (MIP-3, section 14).
pub(crate) fn focus(daemon: &DaemonId, pane: &PaneId) -> Result<(), Refusal> {
    if let Some(tab) = tab_of_pane(pane)
        && reopened_for(&tab, pane.as_str())
    {
        return Ok(());
    }
    {
        let mut session = poison::lock(&SESSION, "session");
        let region = match session.region_holding(daemon, pane) {
            Some(region) => region,
            // Not on screen, which is the interesting half. An agent that finished or is
            // waiting for somebody is most often in a tab no region is showing, so a focus
            // request that refused there would leave the sidebar listing panes nobody can
            // reach - a display, not attention routing.
            None => session.surface(daemon, pane)?,
        };
        session.composition.focus_pane(region, pane.clone());
    }
    publish("focus");
    Ok(())
}

/// Moves the line between two regions, and republishes what that made.
///
/// No daemon is told, and there is nothing to tell one: how a window divides itself between
/// a laptop and a devenv is Muster's own arrangement, and neither daemon knows the other
/// exists. So unlike every other drag in this app, this one settles here.
pub(crate) fn set_region_boundary(left: RegionId, ratio: f32) {
    {
        let mut session = poison::lock(&SESSION, "session");
        session.composition.set_boundary(left, ratio);
    }
    publish("region_boundary");
}

/// Moves the keyboard one pane along, in the window's own reading order.
///
/// The order crosses regions, so a step can land on another daemon - which is the point of
/// showing two of them - and the daemon comes back with the region rather than being assumed
/// to be the one the keyboard just left.
pub(crate) fn step(direction: Step) -> Result<(), String> {
    let stepped = {
        let session = poison::lock(&SESSION, "session");
        session.view().step(direction).and_then(|(region, pane)| {
            Some((session.composition.region(region)?.daemon.clone(), pane))
        })
    };
    let (daemon, pane) = stepped.ok_or_else(|| {
        "this window is showing no panes to step through, so the keyboard stayed where it \
         was. A window with no daemon behind it looks like this, and so does one whose tabs \
         all closed."
            .to_string()
    })?;
    focus(&daemon, &pane).map_err(|refusal| refusal.to_string())
}

/// Evens out the panes around one, in as many requests as it takes.
///
/// Several requests for what is one act to whoever asked, which is the shape this seam usually
/// avoids - and it is unavoidable here, because a backend moves one divider per request and
/// evening a tab out is a statement about all of them at once. Contained by doing the arithmetic
/// in one pure place ([`muster_core::equalize`]) and sending the answer, rather than by growing a
/// backend verb no backend has.
///
/// The region weights go first and are Muster's own, so the tab's parts are already divided by
/// what they hold before the first daemon is asked anything. They are only touched at all when
/// the tab has more than one part: a tab on one machine has a weight that decides nothing, and
/// rewriting it would print a number nobody asked about.
pub(crate) fn equalize(daemon: &DaemonId, pane: &PaneId, evenly: Evenly) -> Result<(), String> {
    let (tab, dividers) = {
        let session = poison::lock(&SESSION, "session");
        let backend = session.backends.get(daemon).ok_or_else(|| {
            format!(
                "this window is not following the daemon {daemon}, so it holds no arrangement to \
                 even out and nothing moved. This is a bug in the core rather than a state to \
                 recover from - a pane was resolved to a machine the session does not have."
            )
        })?;
        let held = poison::lock(&backend.mirror, "mirror");
        let tab = held
            .pane(pane)
            .ok_or_else(|| {
                format!(
                    "the daemon {daemon} no longer holds the pane {pane}, so there is no tab to \
                     even out and nothing moved. A pane that closed while the request was in \
                     flight looks like this; `muster window` says what is still there."
                )
            })?
            .tab
            .clone();
        let layout = held.tree(&tab).ok_or_else(|| {
            format!(
                "the daemon {daemon} no longer holds the tab {tab}, so there is no tree to even \
                 out and nothing moved. A tab that closed while the request was in flight looks \
                 like this; `muster window` says what is still there."
            )
        })?;
        let dividers = equalize::dividers(layout, pane, evenly).ok_or_else(|| {
            format!(
                "the pane {pane} sits in no {} that could be evened out, so nothing moved. A pane \
                 with nothing beside it is in no row and a pane with nothing above or below it is \
                 in no column; `regions[].layout` in `muster window --json` says which.",
                evenly.as_str()
            )
        })?;
        (tab, dividers)
    };

    if evenly == Evenly::Tab {
        let weights = {
            let session = poison::lock(&SESSION, "session");
            let parts: Vec<(RegionId, &DaemonId)> = session
                .composition
                .tab(&tab)
                .into_iter()
                .flat_map(MusterTab::regions)
                .map(|region| (region.id, &region.daemon))
                .collect();
            if parts.len() < 2 {
                Vec::new()
            } else {
                parts
                    .into_iter()
                    .filter_map(|(id, daemon)| {
                        let backend = session.backends.get(daemon)?;
                        let held = poison::lock(&backend.mirror, "mirror");
                        let panes = held.panes_in_tab(&tab).count();
                        // A part the daemon holds no panes in keeps whatever width it had:
                        // a weight of zero is a part nobody can see or grab their way out of,
                        // and the composition refuses one anyway.
                        (panes > 0).then_some((id, panes))
                    })
                    .collect()
            }
        };
        if !weights.is_empty() {
            let mut session = poison::lock(&SESSION, "session");
            for (region, panes) in weights {
                session.composition.set_weight(region, weight_of(panes));
            }
            drop(session);
            publish("equalize");
        }
    }

    // Stops at the first refusal rather than pressing on. What is left behind is a tab part way
    // to even, which is ugly and honest; carrying on would send the rest against a tree the
    // daemon has just said it disagrees about, and land dividers somewhere nobody asked for.
    let asked = dividers.len();
    for (sent, divider) in dividers.into_iter().enumerate() {
        submit(
            daemon,
            &BackendIntent::SetSplitRatio {
                tab: tab.clone(),
                path: divider.path,
                ratio: divider.ratio,
            },
            Keyboard::StaysPut,
        )
        .map_err(|refusal| stopped_evening(daemon, &tab, sent + 1, asked, &refusal))?;
    }
    Ok(())
}

/// Why evening out a tab stopped at one divider, which is the `nth` of `asked`.
fn stopped_evening(
    daemon: &DaemonId,
    tab: &TabId,
    nth: usize,
    asked: usize,
    refusal: &Refusal,
) -> String {
    match refusal {
        Refusal::Unanswered(detail) => format!(
            "the daemon {daemon} did not answer about divider {nth} of {asked} while evening out \
             the tab {tab} ({detail}), so that divider may or may not have moved and the rest \
             were not sent. `muster window --json` says where every pane ended up; asking again \
             from what it now holds is safe, because a divider is sent as a position."
        ),
        refusal => format!(
            "the daemon {daemon} refused divider {nth} of {asked} while evening out the tab \
             {tab} ({refusal}), so the tab is part way there and the rest were not sent. \
             `muster window --json` says where every pane ended up; asking again from what it \
             now holds is safe."
        ),
    }
}

/// A part's share of the window, from how many panes it holds.
///
/// The cast is exact for anything a machine could hold: f32 counts whole numbers exactly to
/// sixteen million, and a window with that many panes has other problems.
#[allow(clippy::cast_precision_loss)]
fn weight_of(panes: usize) -> f32 {
    panes as f32
}

/// Moves the keyboard one tab along, in the order the roster lists them.
///
/// The other axis to [`step`]. That one walks the panes the window is *showing*; this walks
/// every tab every attached daemon holds, so it reaches the ones behind the regions - which
/// no chord could otherwise get to, and which the sidebar was the only door to.
///
/// Crosses daemons for the same reason stepping panes does, and one more: a window's tabs are
/// one list to the person reading them, and a walk that stopped at a machine boundary would
/// leave the other machine's tabs unreachable whenever no region was on it.
pub(crate) fn step_tab(direction: TabStep) -> Result<(), String> {
    let stepped = {
        let session = poison::lock(&SESSION, "session");
        let from = session.composition.showing().cloned();
        session.roster(&session.view()).step(from.as_ref(), direction).map(landing)
    };
    let (daemon, pane) = stepped.ok_or_else(|| {
        "this window is attached to no tabs to step through, so the keyboard stayed where it \
         was. A window whose daemons have not described a session yet looks like this, and so \
         does one whose tabs all closed."
            .to_string()
    })??;
    focus(&daemon, &pane).map_err(|refusal| refusal.to_string())
}

/// Puts one pane where another one is, which is what dropping a row on a row means.
///
/// Which request that becomes is worked out here rather than by the shell, because it is a
/// question about where the two panes are and the mirror is what knows: two panes in one tab
/// exchange places, and a pane dropped on a row in another tab joins that tab behind it. The
/// person dragging made one decision, so there is one intent name for it and one rule.
///
/// Both ends have to be on the daemon named. The sidebar refuses a drop across daemons before
/// it gets here and a CLI caller does not, so for that caller this is the first line rather
/// than the second - and it has to be, because one daemon cannot place a pane beside a pane it
/// does not hold.
pub(crate) fn arrange_pane(daemon: &DaemonId, pane: &PaneId, onto: &PaneId) -> Result<(), Refusal> {
    let intent = {
        let session = poison::lock(&SESSION, "session");
        let backend = session.backends.get(daemon).ok_or_else(|| {
            Refusal::Declined(format!(
                "this window is not following a daemon called {daemon}, so nothing was \
                 rearranged. Either it detached while this was in flight, or the request \
                 named a daemon this window does not have."
            ))
        })?;
        let mirror = poison::lock(&backend.mirror, "mirror");
        let holding = |pane: &PaneId| {
            mirror.pane(pane).map(|held| held.tab.clone()).ok_or_else(|| {
                Refusal::Declined(format!(
                    "{daemon} holds no pane called {pane}, so nothing was rearranged. Either \
                         it closed while this was in flight, or the two panes are on different \
                         machines - a pane is a PTY its daemon owns, so there is no move that \
                         would carry one to the other. `muster window` says which daemon holds \
                         each."
                ))
            })
        };
        let (from, to) = (holding(pane)?, holding(onto)?);
        if from == to {
            BackendIntent::SwapPanes { pane: pane.clone(), with: onto.clone() }
        } else {
            BackendIntent::MovePane {
                pane: pane.clone(),
                to: MoveDestination::Beside { tab: to, after: onto.clone() },
            }
        }
    };
    submit(daemon, &intent, Keyboard::Follows).map(drop)
}

/// Closes a tab and everything in it.
///
/// The only verb here that ends more than it names, which is why it stays beside closing a pane
/// rather than beside renaming one: a tab this window is not showing is a tab whose panes nobody
/// can see, and there is no undo for what was running in them.
pub(crate) fn close_tab(daemon: &DaemonId, tab: &TabId) -> Result<(), Refusal> {
    submit(daemon, &BackendIntent::CloseTab { tab: tab.clone() }, Keyboard::StaysPut).map(drop)
}

/// Calls a tab what somebody wants to call it, on every machine it spans.
///
/// One generation for all of them, higher than any part has had, so every part adopts the name
/// and a part that misses it is renamed when its machine answers again (`catch_up_tab_names`).
/// The first refusal is the answer; the parts that did take the name keep it, and the catch-up
/// carries it to the rest.
pub(crate) fn rename_tab(tab: &TabId, name: Option<&str>) -> Result<(), Refusal> {
    let (parts, generation) = {
        let session = poison::lock(&SESSION, "session");
        let mut parts = Vec::new();
        let mut newest = 0;
        for (daemon, backend) in &session.backends {
            if let Some(part) = poison::lock(&backend.mirror, "mirror").tab(tab) {
                parts.push(daemon.clone());
                newest = newest.max(part.generation);
            }
        }
        (parts, newest + 1)
    };
    if parts.is_empty() {
        return Err(Refusal::NotThere(format!(
            "no machine this window follows holds a tab called {tab}, so nothing was renamed. \
             `muster window` lists the tabs there are."
        )));
    }
    let mut refused = None;
    for daemon in parts {
        let name = name.map(str::to_string);
        let intent = BackendIntent::RenameTab { tab: tab.clone(), name, generation };
        if let Err(refusal) = submit(&daemon, &intent, Keyboard::StaysPut) {
            refused.get_or_insert(refusal);
        }
    }
    refused.map_or(Ok(()), Err)
}

/// Takes a pane out of whatever tab it is in and gives it one of its own.
///
/// Beside [`arrange_pane`] rather than inside it, because the two take different arguments and
/// mean different things to whoever asked: one names where the pane is going and one says it is
/// going nowhere in particular. What they share is the intent, and the adapter is where the two
/// destinations become one request.
///
/// No region and no keyboard move. The tab it makes is listed and not shown, and the keyboard
/// stays where it is, because pulling a pane out of a split is arranging the window rather than
/// going somewhere (`submit`).
pub(crate) fn move_pane_to_new_tab(
    daemon: &DaemonId,
    pane: &PaneId,
    name: Option<String>,
) -> Result<(), Refusal> {
    let intent = BackendIntent::MovePane {
        pane: pane.clone(),
        to: MoveDestination::NewTab { tab: mint_tab(), name },
    };
    submit(daemon, &intent, Keyboard::StaysPut).map(drop)
}

/// A name for a tab this window is about to ask for, unique across every machine it shows.
pub(crate) fn mint_tab() -> TabId {
    let minter = Arc::clone(&poison::lock(&SESSION, "session").minter);
    poison::lock(&minter, "minter").tab()
}

/// Puts a pane into a Muster tab, grouping its machine into that tab if it is not in it yet.
///
/// Stage four of MIP-2, from the outside: the request that makes a tab hold a laptop pane beside
/// a devenv pane. The pane stays on its machine - it is a process - and what moves is which tab
/// it belongs to, which is Muster's to decide because a tab is a grouping Muster made.
///
/// Refused for a tab this window does not hold, which is the only check worth making here: the
/// adapter works out whether this machine already has a half of it, and both answers are ordinary.
///
/// No keyboard move, on the same terms as every other arrangement: putting an agent somewhere is
/// not going there, and taking the keyboard would interrupt whatever is being typed.
pub(crate) fn move_pane_to_tab(
    daemon: &DaemonId,
    pane: &PaneId,
    tab: &TabId,
) -> Result<(), Refusal> {
    {
        let session = poison::lock(&SESSION, "session");
        if session.composition.tab(tab).is_none() {
            return Err(Refusal::NotThere(format!(
                "this window holds no tab called {tab}, so {pane} was not moved. Either it \
                 closed while this was in flight, or the name came from another window - \
                 `muster window` lists the tabs this one holds."
            )));
        }
    }
    let intent = BackendIntent::MovePane {
        pane: pane.clone(),
        to: MoveDestination::Tab { tab: tab.clone() },
    };
    submit(daemon, &intent, Keyboard::StaysPut).map(drop)
}

/// Brings a named tab on screen, landing the keyboard on its first pane.
///
/// The mouse's half of what `next_tab` does with the keyboard, through the same [`landing`]
/// rule so that the two agree about where a tab is entered.
pub(crate) fn focus_tab(tab: &TabId) -> Result<(), String> {
    if reopened_for(tab, tab.as_str()) {
        return Ok(());
    }
    let found = {
        let session = poison::lock(&SESSION, "session");
        match session.roster(&session.view()).tabs().find(|held| &held.id == tab) {
            Some(held) => landing(held),
            None => Err(format!(
                "this window holds no tab called {tab}, so the keyboard stayed where it was. \
                 Most likely it closed while the click was in flight."
            )),
        }
    };
    let (daemon, pane) = found?;
    focus(&daemon, &pane).map_err(|refusal| refusal.to_string())
}

/// Hands a tab to a window: this one when `window` is empty, or another by pid or by name.
///
/// A write to the record every window shares, so any window can make it and none has to be
/// asked. The window that had the tab hears the record move and lets it go; the one that has it
/// now hears the same and lists it. This one acts at once on its own half, because it may be
/// either of them.
///
/// Brought here by naming no window, the tab comes on screen, because whoever asked is looking at
/// this window. Moved to a window by name - this one included - it joins the end of that window's
/// list without coming on screen: a CLI outside every pane sends it to whichever window answers
/// first, so what it does cannot depend on which window that was.
pub(crate) fn move_tab(tab: Option<TabId>, window: &str) -> Result<(), Refusal> {
    let bring_here = window.is_empty();
    let tab = {
        let mut session = poison::lock(&SESSION, "session");
        let tab = match tab {
            Some(tab) => tab,
            None => session.composition.showing().cloned().ok_or_else(|| {
                Refusal::Declined(
                    "this window is showing no tab, so there was none to move. Name one with \
                     --tab."
                        .to_string(),
                )
            })?,
        };
        let described = session
            .backends
            .values()
            .any(|backend| poison::lock(&backend.mirror, "mirror").tab(&tab).is_some());
        if bring_here && !described {
            return Err(Refusal::NotThere(format!(
                "no daemon this window is following holds a tab called {tab}, so nothing was \
                 moved. `muster window` lists the tabs there are."
            )));
        }
        // A move naming its window is a write to the record every window shares, so a tab on a
        // machine this window does not follow can still be sent: the record says it exists.
        let known = described || session.holding.holders().holder(&tab).is_some();
        if !known {
            return Err(Refusal::NotThere(format!(
                "no window knows a tab called {tab}, so nothing was moved. `muster window` lists \
                 the tabs there are."
            )));
        }
        let to = session.holding.destination(window)?;
        let to_me = &to == session.holding.me();
        let from = session.holding.holders().holder(&tab).map(ToString::to_string);
        session.holding.give(&tab, &to);
        log::info(
            "tab.moved",
            fields! {
                "tab" => tab.to_string(),
                "from" => from.unwrap_or_default(),
                "to" => to.to_string(),
            },
        );
        if to_me {
            session.composition.hold(tab.clone());
        } else {
            session.composition.let_go(&tab);
        }
        tab
    };
    reconcile_every_daemon();
    if bring_here {
        return focus_tab(&tab).map_err(Refusal::Declined);
    }
    publish("move_tab");
    Ok(())
}

/// Why a pane in another window's tab was not shown here.
fn taken_elsewhere(pane: &PaneId, tab: &TabId, window: &WindowName) -> Refusal {
    Refusal::Declined(format!(
        "{pane} is in {tab}, which is in another window ({window}), so the keyboard stayed where \
         it was. Showing it here would take its terminals from that window."
    ))
}

/// Asks for a closed window to be opened again when going somewhere means going into it.
///
/// A closed window keeps its tabs and their agents keep running, so a notification about one of
/// them, or `muster tab focus` naming one, is somebody going to that window. Says whether it
/// asked. An open window's tab never reaches here from a caller - it is carried to that window
/// (`forward`) - and one that does is refused further on, since showing it here would take its
/// terminals.
fn reopened_for(tab: &TabId, show: &str) -> bool {
    let (me, window) = {
        let session = poison::lock(&SESSION, "session");
        let Some(window) = session.holding.elsewhere(tab).cloned() else { return false };
        (session.holding.me().clone(), window)
    };
    if crate::holding::is_open(&me, &window) {
        return false;
    }
    if !poison::lock(&SESSION, "session").holding.ask_to_reopen(&window.name) {
        log::info(
            "window.reopen.already_asked",
            fields! { "window" => window.name.to_string(), "show" => show },
        );
        return true;
    }
    log::info(
        "window.reopen.asked",
        fields! { "window" => window.name.to_string(), "show" => show },
    );
    ffi::emit(&Event {
        payload: Some(event::Payload::ReopenWindow(ReopenWindow {
            name: window.name.to_string(),
            show: show.to_string(),
        })),
    });
    true
}

/// Puts the keyboard on whatever the numbered chord for `place` names.
///
/// What ⌘1 to ⌘9 mean, and under `numbered_chords = "panes"` that is a pane at a place in the
/// window's pane order and nothing else happens. A place past the last one is refused by name
/// rather than clamped to the last: a chord that lands somewhere different every time a pane
/// opens is worse than a chord that does nothing until there is something to do it to.
///
/// Under `tab_then_pane` the same request means the second press as readily as the first, and
/// which one it is depends on what the press before it did. That is the prototype's whole
/// cost, and it is paid here rather than in the shell because on macOS a menu item cannot hold
/// two-stage state - the round trip into this side is the only place both presses meet.
///
/// **Reaching a tab acts immediately.** ⌘2 goes to the second tab there and then, landing
/// through the same [`landing`] rule a click on a caption and `next_tab` use, so three ways of
/// entering a tab agree about where you arrive. A chord that did nothing until the next one
/// arrived would be indistinguishable from a dead key, and if the tab was all that was wanted
/// you are already there.
///
/// No `landing` step for a pane, unlike a tab: a pane names itself, where a tab has to
/// nominate one of its own. Reaching a tab nothing is showing still works either way, because
/// [`focus`] surfaces the tab holding the pane.
pub(crate) fn focus_pane_at(place: usize) -> Result<(), String> {
    let found = {
        let mut session = poison::lock(&SESSION, "session");
        let roster = session.roster(&session.view());
        let numbering = session.numbering(&roster);
        if let Some(landing) = roster.numbered(&numbering, place) {
            let pane = landing.pane();
            let found = (pane.key.daemon.clone(), pane.key.pane.clone());
            // Set from the landing either way, so that a press onto a pane starts the next one
            // over: three ⌘2s are the second tab, its second pane, and the second tab again.
            session.armed = landing.named();
            Ok(found)
        } else {
            session.armed = None;
            Err(nothing_numbered(&roster, &numbering, place))
        }
    };
    let (daemon, pane) = found?;
    focus(&daemon, &pane).map_err(|refusal| refusal.to_string())
}

/// Why a numbered chord reached nothing, said in the terms of whatever it was counting.
///
/// One refusal per branch rather than one for all three, because "this window holds 2 panes"
/// in front of somebody whose ⌘9 was asking about tabs sends them looking in the wrong place.
fn nothing_numbered(roster: &Roster, numbering: &Numbering, place: usize) -> String {
    match numbering {
        Numbering::Panes => format!(
            "this window holds {} panes, so there is no pane {place} to go to and the keyboard \
             stayed where it was.",
            roster.panes().count()
        ),
        Numbering::Tabs => format!(
            "this window holds {} tabs, so there is no tab {place} to go to and the keyboard \
             stayed where it was. ⌘1 to ⌘9 are naming tabs because `numbered_chords` is \
             `tab_then_pane`; under `panes` they would be naming panes.",
            roster.tabs().count()
        ),
        Numbering::PanesIn(key) => {
            let held = roster.tabs().find(|tab| &tab.id == key).map_or(0, |tab| tab.panes.len());
            format!(
                "{key} holds {held} panes, so there is no pane {place} in it and the keyboard \
                 stayed where it was. This was the second press of a `tab_then_pane` chord, so \
                 the number was counting inside that tab rather than down the whole window."
            )
        }
    }
}

/// Forgets a tab a numbered chord had named, and says so on the way out.
///
/// Called for every request that changes anything, so that the second half of a `tab_then_pane`
/// chord has to be the very next thing that happens. See [`crate::handler`] for the rule, and
/// why it is one line there rather than a list of callers here.
///
/// Announces rather than only forgetting, because forgetting moves the numbers back onto the
/// tabs and the sidebar is drawing them. Most of the requests that land here - a keystroke, a
/// close, a drag - never publish, so an arm dropped silently would leave a list of numbers
/// nothing can press. It runs at most once per armed chord: the second call finds nothing.
pub(crate) fn disarm() {
    let held = {
        let mut session = poison::lock(&SESSION, "session");
        session.armed.take().is_some()
    };
    if held {
        announce_roster();
    }
}

/// Says what exists and what reaches it, without settling anything else.
///
/// The narrow half of [`publish`], for the callers that have changed which rows carry numbers
/// and nothing else. Going through `publish` would reconcile every daemon and save the
/// composition on a keystroke, which is a lot of work to say that a number moved.
pub(crate) fn announce_roster() {
    let _publishing = poison::lock(&PUBLISHING, "publishing");
    let (roster, numbering, message) = {
        let mut session = poison::lock(&SESSION, "session");
        let roster = session.roster(&session.view());
        let numbering = session.numbering(&roster);
        let message = convert::roster(&roster, &numbering);
        let unseen = session.sent.roster(&message);
        (roster, numbering, unseen.then_some(message))
    };
    let Some(message) = message else { return };
    // The same line `publish` writes, because the question a run log has to answer about this
    // is "which rows carried numbers, and when" - and half the answers arriving on a line that
    // says nothing would make the log worse than no log for exactly the feature it is for.
    log::info(
        "roster.numbering",
        fields! {
            "numbering" => describe_numbering(&numbering),
            "tabs" => roster.tabs().count().to_string(),
            "panes" => roster.panes().count().to_string(),
        },
    );
    ffi::emit(&Event { payload: Some(event::Payload::RosterChanged(message)) });
}

/// One numbering, as a log line says it.
fn describe_numbering(numbering: &Numbering) -> String {
    match numbering {
        Numbering::Panes => "panes".to_string(),
        Numbering::Tabs => "tabs".to_string(),
        Numbering::PanesIn(key) => format!("panes in {key}"),
    }
}

/// Where the keyboard lands when a tab is shown.
///
/// The tab's first pane in the roster's own order, so that going to a tab and reading its
/// rows agree about which one comes first. The daemon keeps no focus to ask instead: where
/// this window's keyboard goes is this window's to decide.
///
/// Names the pane rather than focusing it, because [`focus`] takes the session lock and every
/// caller here is holding it.
fn landing(tab: &RosterTab) -> Result<(DaemonId, PaneId), String> {
    let pane = tab.panes.first().ok_or_else(|| {
        format!(
            "{} holds no panes, so there is nothing for the keyboard to land on. Most likely \
             they closed while this was in flight.",
            tab.id
        )
    })?;
    Ok((pane.key.daemon.clone(), pane.key.pane.clone()))
}

/// The pane this window's keyboard feeds, named.
pub(crate) fn focused_pane() -> Option<PaneId> {
    let session = poison::lock(&SESSION, "session");
    session.composition.focused_region()?.pane.clone()
}

/// Which daemon holds the pane Muster calls this, if any followed one does.
///
/// What lets a caller name a pane and nothing else. A name is unique across every attached
/// machine, so saying which machine holds it would be asking for something the caller has no
/// way to know and no reason to.
pub(crate) fn daemon_holding(pane: &PaneId) -> Option<DaemonId> {
    locate(pane).map(|(daemon, ..)| daemon)
}

/// The tab a pane is in, on whichever machine holds it.
pub(crate) fn tab_of_pane(pane: &PaneId) -> Option<TabId> {
    locate(pane).map(|(_, tab)| tab)
}

/// The other window holding a tab, if it is open.
///
/// Dialed after the session is let go: the other window may be carrying a request to this one at
/// the same moment, and answering it needs this lock.
pub(crate) fn open_window_holding(tab: &TabId) -> Option<HeldWindow> {
    let (me, window) = {
        let session = poison::lock(&SESSION, "session");
        (session.holding.me().clone(), session.holding.elsewhere(tab).cloned()?)
    };
    crate::holding::is_open(&me, &window).then_some(window)
}

/// This window's name, as the record of which window holds each tab spells it.
pub(crate) fn window_name() -> String {
    poison::lock(&SESSION, "session").holding.me().to_string()
}

/// Brings a window to the front, because somebody went to one of its tabs from another window:
/// this one for `pid` 0, or the window with that process, which this one hands activation to.
pub(crate) fn raise_window(pid: u32) {
    ffi::emit(&Event { payload: Some(event::Payload::RaiseWindow(RaiseWindow { pid })) });
}

/// The daemon this window's keyboard is on.
///
/// What a request naming no daemon means, for the same reason an empty pane id means the
/// focused pane: a menu item is about what is in front of the user and has nothing else to
/// say.
pub(crate) fn focused_daemon() -> Option<DaemonId> {
    let session = poison::lock(&SESSION, "session");
    session.composition.focused_region().map(|region| region.daemon.clone())
}

/// The pane this window's keyboard feeds on one named machine.
///
/// What a request that names a machine and no pane means, in three steps: the pane the keyboard
/// is on if that is on the named machine, then that machine's half of the tab on screen, and
/// failing both that machine's first pane anywhere in the window. So `--daemon` reaches the pane
/// somebody would be typing into if they went there.
///
/// The third step is what a window showing one tab needs. At most one machine's panes are drawn
/// unless somebody has grouped two, so a machine whose tabs are all in the background is the
/// ordinary state rather than a rare one - and refusing there would leave `--daemon` working
/// only for whichever machine happened to be on screen.
///
/// Read from the composition, because the daemon keeps no focus of its own: where this window's
/// requests land is this window's to decide.
pub(crate) fn focused_pane_on(daemon: &DaemonId) -> Option<PaneId> {
    let session = poison::lock(&SESSION, "session");
    let on_screen = session
        .composition
        .focused_region()
        .filter(|region| &region.daemon == daemon)
        .or_else(|| session.composition.regions().find(|region| &region.daemon == daemon))
        .and_then(|region| region.pane.clone());
    if on_screen.is_some() {
        return on_screen;
    }
    // A machine whose tabs are all in the background, which is now the ordinary state rather
    // than a rare one: a window shows one tab, so at most one machine's panes are drawn unless
    // somebody has grouped two. `--daemon` names a machine directly, and a caller saying it
    // means "act over there" rather than "act over there if it happens to be on screen".
    session
        .composition
        .tabs()
        .flat_map(MusterTab::regions)
        .find(|region| &region.daemon == daemon)?
        .pane
        .clone()
}

/// Whether this window is holding any tab with panes on this machine.
///
/// Not the same question as whether it is *showing* one: a tab in the background is still a
/// tab this window can act on, and a machine that has none is one nothing in the window can
/// reach.
pub(crate) fn holding_nothing(daemon: &DaemonId) -> bool {
    let session = poison::lock(&SESSION, "session");
    !session.composition.tabs().any(|tab| tab.daemons().any(|holding| holding == daemon))
}

/// Whether this window is following a daemon by this name.
///
/// What lets a name somebody typed be refused by name rather than reaching [`submit`], which
/// answers for an unfollowed daemon with a message about a bug in the core.
pub(crate) fn is_following(daemon: &DaemonId) -> bool {
    let session = poison::lock(&SESSION, "session");
    session.backends.contains_key(daemon)
}

/// Every daemon this window is following, in the order the window shows them.
///
/// For naming the machines there are when somebody has named one there is not. The window's
/// order rather than the config file's, so the list reads the way `muster window` prints it.
pub(crate) fn attached_daemons() -> Vec<DaemonId> {
    let session = poison::lock(&SESSION, "session");
    session.composition.daemons().map(|daemon| daemon.id.clone()).collect()
}

/// Everything about this window at one moment, for a caller that gets no events.
///
/// The shell is told what changed as it changes and builds its own picture; a script runs for
/// one command and has nothing to build on, so it has to be able to ask. Same builders as
/// [`publish`], so the answer cannot contradict what the window is drawing.
pub(crate) struct WindowNow {
    pub view: View,
    pub roster: Roster,
    /// What the numbered chords name at this moment, so the answer carries the same numbers
    /// the sidebar is drawing rather than leaving a reader to guess the scheme.
    pub numbering: Numbering,
    /// Every pane, with the state the window would paint for it - which is not always the one
    /// the daemon reported: `done` is this window's answer rather than the daemon's, because a
    /// daemon cannot see which window has been looked at.
    pub agents: Vec<PaneAgent>,
    /// Each followed daemon: how much of its truth Muster has, and enough about it to decide
    /// deliberately what happens to it.
    pub daemons: Vec<Machine>,
    /// This window's name, as the record of which window holds each tab spells it.
    pub name: String,
    /// Every other window, open or closed, with the tabs it holds.
    pub others: Vec<OtherWindow>,
}

/// Another window, as this one can describe it.
#[derive(Debug)]
pub(crate) struct OtherWindow {
    pub name: String,
    /// Zero once it has closed, or when its socket stopped answering.
    pub pid: u32,
    /// Its tabs, described from this window's copy of each daemon.
    pub roster: Roster,
}

/// One pane's agent, as this window paints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneAgent {
    pub pane: PaneKey,
    pub state: AgentState,
    /// When the agent last changed state, in milliseconds since the epoch. Zero for a pane
    /// nothing has stamped, which a window only holds between a daemon describing a pane and
    /// that change reaching `report`.
    pub since_ms: i64,
}

/// How much of one daemon's truth the window has, as the shell and a watch are told it.
#[derive(Debug, Clone)]
pub(crate) struct DaemonHealth {
    pub daemon: DaemonId,
    pub health: Health,
    /// Why, when it is not connected.
    pub detail: String,
}

/// One machine this window is attached to, as anything outside the core reads it.
///
/// Muster is the only thing that can answer this. A socket can be asked what it holds and the
/// OS can be asked which process holds a socket, and nothing gets from one to the other - so
/// the pairing is Muster's to keep, because Muster either started the daemon or chose to
/// attach to it (kan a_28YghIUw2).
#[derive(Debug, Clone)]
pub(crate) struct Machine {
    pub daemon: DaemonId,
    /// Where it runs, or `None` for this machine.
    pub host: Option<String>,
    pub socket_path: String,
    pub started: bool,
    pub health: Health,
    pub detail: String,
    /// Every directory its panes are in, deduplicated and in order.
    ///
    /// What makes a process recognisable at the moment somebody is deciding whether to end it.
    /// A count says how much would be lost; a directory says what.
    pub directories: Vec<String>,
    pub panes: usize,
}

/// Every pane every followed daemon holds, as this window paints it.
///
/// The part of [`window`] a watch starts from, without building a view and a roster it would
/// throw away.
pub(crate) fn agents() -> Vec<PaneAgent> {
    poison::lock(&SESSION, "session").agents()
}

/// How much of each followed daemon's truth the window has, read off its mirror.
///
/// The mirror rather than the last health announced, because every change to a mirror's health
/// is announced after it is written - so a watch that reads this after registering hears any
/// change it missed here on its channel instead.
pub(crate) fn daemon_health() -> Vec<DaemonHealth> {
    let session = poison::lock(&SESSION, "session");
    session
        .backends
        .iter()
        .map(|(daemon, backend)| {
            let mirror = poison::lock(&backend.mirror, "mirror");
            DaemonHealth {
                daemon: daemon.clone(),
                health: mirror.health(),
                detail: mirror.health_detail().to_string(),
            }
        })
        .collect()
}

pub(crate) fn window() -> WindowNow {
    let session = poison::lock(&SESSION, "session");
    // No reconcile, unlike `publish`. This is a read: a caller asking what the window shows
    // must not be able to move the keyboard or open a region by asking, and anything that
    // needed reconciling has already published.
    let view = session.view();
    let roster = session.roster(&view);
    let numbering = session.numbering(&roster);

    let agents = session.agents();
    let mut daemons = Vec::new();
    for (id, backend) in &session.backends {
        let mirror = poison::lock(&backend.mirror, "mirror");
        let mut directories: Vec<String> = Vec::new();
        let mut panes = 0usize;
        for pane in mirror.panes() {
            panes += 1;
            if !pane.cwd.is_empty() && !directories.contains(&pane.cwd) {
                directories.push(pane.cwd.clone());
            }
        }
        daemons.push(Machine {
            daemon: id.clone(),
            host: backend.tunnel.as_ref().map(|tunnel| tunnel.host().to_string()),
            socket_path: backend.socket_path.clone(),
            started: backend.started,
            health: mirror.health(),
            detail: mirror.health_detail().to_string(),
            directories,
            panes,
        });
    }

    let name = session.holding.me().to_string();
    let me = session.holding.me().clone();
    let others = session.other_windows();
    drop(session);
    // Dialed with the session let go, for the reason `open_window_holding` gives.
    let others = others
        .into_iter()
        .map(|(window, roster)| OtherWindow {
            name: window.name.to_string(),
            pid: if crate::holding::is_open(&me, &window) { window.pid } else { 0 },
            roster,
        })
        .collect();

    WindowNow { view, roster, numbering, agents, daemons, name, others }
}

/// Starts following every daemon a config file named.
///
/// No regions yet. Which tab a region shows depends on where the pane in `argv` turned out to
/// live, and that is not known until [`attach`] has asked - so opening one here would mean
/// opening a second one a moment later and closing the first.
///
/// A daemon that will not attach is logged and skipped rather than fatal. One unreachable
/// devenv should cost its own panes and nothing else, and a window that refused to open
/// because a container was down would be worse than no window at all.
pub(crate) fn follow_configured(config: &Config) {
    for daemon in &config.daemons {
        if let Err(refusal) = attach_daemon(daemon) {
            log::error(
                "daemon.unavailable",
                fields! {
                    "daemon" => daemon.id.to_string(),
                    "detail" => refusal,
                    "impact" => "this daemon's panes are absent from the window; every other \
                                 daemon in the config is unaffected",
                },
            );
        }
    }
}

/// How long a window opening onto a daemon waits for its first snapshot before carrying on
/// without it. The daemon's panes arrive on their own when it answers.
const FIRST_SNAPSHOT: std::time::Duration = std::time::Duration::from_secs(10);

fn attach_daemon(daemon: &Daemon) -> Result<(), String> {
    let reached = reach(&daemon.id, &daemon.endpoint)?;
    log::info(
        "daemon.attached",
        fields! {
            "daemon" => daemon.id.to_string(),
            "socket" => reached.socket_path.clone(),
            "remote" => reached.tunnel.as_ref().map_or("", Tunnel::host).to_string(),
            "started" => reached.started,
        },
    );
    let connection = poison::lock(&SESSION, "session").follow(daemon, reached)?;
    // Not under the session's lock: announcing the snapshot takes it.
    if let Some(connection) = connection
        && !connection.wait_for_snapshot(FIRST_SNAPSHOT)
    {
        // Let go of it rather than keep following it. A daemon that never answered is not
        // one this window is showing, and kept, it would count as something followed - so a
        // window whose every configured daemon is silent would open onto nothing instead of
        // saying which of them did not answer.
        let removed = {
            let mut session = poison::lock(&SESSION, "session");
            session.composition.detach_daemon(&daemon.id);
            session.backends.remove(&daemon.id)
        };
        // Dropped with the lock released: dropping a follower joins its thread, which can be
        // part way through connecting, and every other daemon's events need the lock meanwhile.
        drop(removed);
        return Err(format!(
            "the daemon {} did not send its state within {}s",
            daemon.id,
            FIRST_SNAPSHOT.as_secs()
        ));
    }
    Ok(())
}

/// Why a pane could not be attached.
pub(crate) enum AttachError {
    Unreachable(String),
    NoSuchPane { pane: String, held: usize, dropped: usize },
    NoChannel(String),
}

/// Opens this window onto whatever the daemons hold.
///
/// What a bare `muster` means. No pane is named, so nothing decides which tab beyond the saved
/// arrangement and, failing that, the first tab this window is free to open onto.
///
/// Three steps, and each is the reason the next can be simple: be following something, put back
/// what was left, and settle what to show - which for a window somebody asked for means keeping
/// off the tabs another window has, and for a window with nothing at all means asking a machine
/// for a tab. That last one is what a fresh machine needs, where Muster has just started a
/// daemon that has not answered anything yet.
pub(crate) fn open() -> Result<(), String> {
    follow_implicitly_if_nothing_else()?;
    restore_presentation();
    restore_font_sizes();
    say_this_window_is_open();
    reopen_what_was_left();
    // After the file has been read and before anything writes over it. Everything above reads
    // the arrangement; everything from here on is entitled to replace it - which is why this
    // sits above the last two steps rather than below them, and it has to. Asking for a tab
    // is answered by the daemon on its own thread, and the region for it is opened
    // by the standing rule when that answer lands; a window that had not yet said it was open
    // would turn that rule off and wait forever for a region nothing else will make.
    mark_opened();
    settle_what_the_window_shows();
    // A window just opened has been sent nothing, whatever an earlier one was.
    poison::lock(&SESSION, "session").sent = Sent::default();
    publish("open");
    show_what_was_asked_for();
    Ok(())
}

/// What this launch was asked to go to, if anything: a closed window reopened onto one of its
/// tabs, because somebody went to it from another window.
///
/// After everything else in `open`, so the tab is one this window has restored. A name that is
/// no longer there is logged rather than refused: the window has opened, and wherever it was
/// left is a fine place for it to be.
static SHOW: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_show(show: &str) {
    *poison::lock(&SHOW, "show") = (!show.is_empty()).then(|| show.to_string());
}

fn show_what_was_asked_for() {
    let Some(show) = poison::lock(&SHOW, "show").take() else { return };
    let tab = TabId::new(&show);
    let went = if daemon_holding_tab(&tab).is_some() {
        focus_tab(&tab)
    } else {
        let pane = PaneId::new(&show);
        match daemon_holding(&pane) {
            Some(daemon) => focus(&daemon, &pane).map_err(|refusal| refusal.to_string()),
            None => Err(format!("no daemon holds a pane or tab called {show}")),
        }
    };
    if let Err(detail) = went {
        log::warn(
            "window.show.failed",
            fields! {
                "show" => &show,
                "detail" => detail,
                "impact" => "the window opened where it was left rather than on what it was \
                             reopened for",
                "check" => "whether that tab or pane closed while the window was opening",
            },
        );
    }
}

/// Puts back the window's own chrome, and tells the shell either way.
///
/// Separate from the regions, and ahead of them, because it survives conditions they do not.
/// A saved region is a wish about a session that may be gone, so restoring one can come to
/// nothing; nobody else has an opinion about whether a list was open, so this always applies.
/// Folding it into `reopen_what_was_left` would tie it to that function's early return, and a
/// person who put the sidebar away would find it back whenever their tabs did not survive.
///
/// Announced unconditionally, including when there was nothing to read, so the shell is told
/// the default rather than holding one.
fn restore_presentation() {
    let saved = saved_presentation();
    let presentation = {
        let mut session = poison::lock(&SESSION, "session");
        // The frame is the one field the shell may already have answered. It asks where to open
        // before showing the window and reports where it actually opened, and either of those
        // can land before this runs - so a wholesale assignment here would throw away the
        // rectangle the window is currently at and write the wish back over the answer. Keeping
        // what is already set makes the two orders agree, which is what `open()` overwriting
        // presentation wholesale has caught out before.
        let presentation = match session.presentation.frame {
            Some(_) => {
                saved.with_frame(session.presentation.frame, session.presentation.full_screen)
            }
            None => saved,
        };
        session.presentation = presentation;
        presentation
    };
    // Then let anything already wrong have its say. A config refused during `Startup` raised
    // its problem before this ran, so without this the saved answer would quietly win and a
    // window would come back with the roster away and nowhere to report a broken file. It
    // announces for itself when it moves anything, which is why this one is conditional.
    if !reconcile_sidebar_with_problems() {
        announce_presentation(presentation);
    }
}

/// Puts back how big each pane's text was.
///
/// Beside the chrome rather than folded into it, because the two are different answers: the
/// chrome is one statement about the window and this is a row per pane somebody sized.
///
/// Restored whole and unchecked, unlike a region. An entry naming a pane that is gone costs a
/// row nobody reads until the next publish drops it; a region naming a tab that is gone is a
/// square on screen that never fills in. The pruning belongs where a daemon that is actually
/// answering can say what it still holds, which is `forget_what_closed`.
///
/// Nothing is announced. Every size here reaches the shell on the pane it belongs to, and the
/// publish that ends `open()` is what carries them.
fn restore_font_sizes() {
    let Some(saved) = saved_arrangement() else { return };
    if saved.font_sizes.entries().next().is_none() {
        return;
    }
    log::info(
        "state.font_size.restored",
        fields! { "panes" => saved.font_sizes.entries().count().to_string() },
    );
    poison::lock(&SESSION, "session").font_sizes = saved.font_sizes;
}

/// Tells the other windows this one is open, and brings this window's idea of who holds what up
/// to date.
///
/// Before the restore, which reads the answer: a tab another window took while this one was
/// closed is not this window's to reopen.
fn say_this_window_is_open() {
    {
        let mut session = poison::lock(&SESSION, "session");
        if session.holding.has_opened() {
            return;
        }
        // A daemon still restoring has not described every tab it will hold.
        let mut answered = BTreeSet::new();
        let mut described = BTreeSet::new();
        for (daemon, backend) in &session.backends {
            let mirror = poison::lock(&backend.mirror, "mirror");
            if mirror.health() == Health::Connected && !mirror.restoring() {
                answered.insert(daemon.clone());
                described.extend(mirror.tabs().map(|tab| tab.id.clone()));
            }
        }
        let followed = session.backends.keys().cloned().collect();
        session.holding.follow(followed);
        session.holding.open(&answered, |tab| described.contains(tab));
    }
    // The daemons described their tabs before this window had said it was open, when it could
    // not be the one to take them. On a first launch that is every tab there is.
    take_what_nobody_holds();
}

/// Takes the tabs nobody holds that are this window's to take, on every machine it follows, and
/// says whether it took any.
///
/// What a tab nobody holds needs asked again whenever the answer to "whose is it" may have become
/// this window: when it says it is open, and when it comes to the front. Otherwise nothing asks
/// until the daemon next says something, which on a quiet session is never.
fn take_what_nobody_holds() -> bool {
    let mut session = poison::lock(&SESSION, "session");
    let daemons: Vec<DaemonId> = session.backends.keys().cloned().collect();
    let mut took = false;
    for daemon in &daemons {
        if session.settle_holding(daemon) {
            session.reconcile(daemon);
            took = true;
        }
    }
    took
}

/// Puts back the regions this window was showing when it last closed.
///
/// Before the two rules under it rather than instead of them, which is what makes this an
/// addition and not a special case: a daemon whose saved regions all turned out to be gone
/// falls through to getting a region of its own, and a window where every daemon did falls
/// through to asking for a tab. So a first launch, a launch after a reboot took
/// everything, and a launch onto a session still running are one path with different amounts
/// of it doing anything.
///
/// Checked against the mirror, which by now holds each attached daemon's snapshot: a saved
/// region is a wish, and a tab nobody holds any more would render as a square that never
/// fills in.
fn reopen_what_was_left() {
    let mut session = poison::lock(&SESSION, "session");
    // Tabs given to this window while it was closed are its own as much as the ones it was left
    // on, and come after them.
    let given: Vec<TabId> =
        session.holding.holders().held_by(session.holding.me()).cloned().collect();
    for tab in given {
        session.composition.hold(tab);
    }
    let Some(saved) = saved_arrangement() else { return };

    // Only the tabs that are still this window's. One another window has taken since is that
    // window's now, and one nobody holds - every tab, the first launch after holding was written
    // down - is taken here, which is how a window that was alone comes back exactly as it was.
    let listed: Vec<TabId> = saved.tabs.iter().map(|tab| tab.id.clone()).collect();
    session.holding.keep(&listed);
    let restorable = saved.restorable(|daemon, tab| {
        session.holding.holds(tab)
            && session
                .backends
                .get(daemon)
                .is_some_and(|backend| poison::lock(&backend.mirror, "mirror").tab(tab).is_some())
    });
    if restorable.tabs.is_empty() {
        return;
    }

    let mut regions = 0usize;
    for tab in &restorable.tabs {
        for region in &tab.regions {
            // Idempotent per machine per tab, so a file holding the same region twice heals
            // here rather than opening a second one. Two regions on one machine's half of a
            // tab render the same pane twice, and only one of the two surfaces can have the
            // pane - the other is refused, and becomes a panel that cannot be closed, because
            // closing it would close the pane the live one is using (kan a_2Ht74jTXV).
            let Some(id) = session.composition.open_region(&region.daemon, tab.id.clone()) else {
                continue;
            };
            session.composition.set_weight(id, region.weight);
            if let Some(pane) = &region.pane {
                session.composition.focus_pane(id, pane.clone());
            }
            if region.keyboard {
                session.composition.focus_region(id);
            }
            regions += 1;
        }
    }
    if let Some(showing) = &restorable.showing {
        session.composition.show(showing);
    }

    log::info(
        "composition.restored",
        fields! {
            "tabs" => restorable.tabs.len().to_string(),
            "regions" => regions.to_string(),
            "showing" => restorable.showing.as_ref().map(ToString::to_string).unwrap_or_default(),
            "dropped" => (saved.tabs.len() - restorable.tabs.len()).to_string(),
        },
    );
}

/// Attaches the daemon on this machine when no config file named any.
///
/// Recorded as the wish that produced it - Muster's own daemon, wherever that turns out to be
/// - rather than as the path that answered today.
///
/// A config that named daemons is answered here even when none of them could be reached, and
/// the answer is a refusal rather than a substitute. Standing in for a daemon somebody named
/// is not a lesser version of showing it: the window renders another session's panes under the
/// configured daemon's id, and nothing on screen says which session it is looking at. Under
/// load that is how the suite's own tests once reached a developer's live herdr (kan a_2L19sAmLZ),
/// and a person whose devenv is briefly slow would get the same window with no way to tell.
///
/// `follow_configured` still skips a daemon that will not attach, and for the reason it gives:
/// one unreachable devenv should cost its own panes and nothing else. This is the case it does
/// not cover - *every* named daemon failing - where there are no other panes for it to be
/// costing nothing.
fn follow_implicitly_if_nothing_else() -> Result<(), String> {
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
    attach_daemon(&implicit)
}

/// Every daemon a config file named, said the way the file named it.
///
/// The endpoint rather than the id, because the id is the reader's own word and the endpoint is
/// the part they can check.
fn named_daemons() -> Vec<String> {
    let configured = poison::lock(&CONFIGURED_DAEMONS, "settings");
    configured
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|daemon| match &daemon.endpoint {
            Endpoint::Local { socket_path: None } => {
                format!("{} on this machine", daemon.id)
            }
            Endpoint::Local { socket_path: Some(path) } => format!("{} at {path}", daemon.id),
            Endpoint::Ssh { host, .. } => format!("{} on {host}", daemon.id),
        })
        .collect()
}

/// Asks for one tab when this window has no tab it may open onto.
///
/// The one rule that makes a window out of nothing, and the only one left: a window is not a
/// window if it is showing nothing, so something has to fill it. It picks the first local
/// machine, because a remote one is somebody else's and choosing it uninvited is a bigger claim
/// than filling a window.
///
/// **Once per launch, per machine, and never again.** Nothing takes a machine back out of
/// `tabs_asked_of`, so a machine whose panes all close later stays empty - which is the
/// answer `a_2I6h18OU6` settled on, now that `muster pane new --daemon <id>` and the machines at
/// the foot of the agent list are both ways back in. The rule this replaces gave every attached
/// machine a column and refilled it the moment it emptied, so somebody finished with a devenv
/// for the afternoon got a fresh ssh shell every time they closed one.
///
/// Nothing is asked of a machine still restoring what it held before it last stopped. Its tabs
/// are on their way, and a tab asked for now would be one more beside them; it is asked once
/// it says it has finished, if the window is still empty then.
fn open_a_tab_if_the_window_is_empty() {
    let empty = {
        let session = poison::lock(&SESSION, "session");
        session.composition.showing().is_none()
    };
    if !empty {
        return;
    }

    let Some(daemon) = first_local_daemon().filter(has_spoken) else {
        log::info(
            "window.empty",
            fields! {
                "impact" => "this window shows nothing yet, because no machine on this one has \
                             both answered and offered a tab it may open onto",
                "check" => "nothing, on the way up - this runs again on every machine's first \
                            snapshot. If it stays this way, `muster pane new --daemon <id>` \
                            names a machine directly, and so does the row for it at the foot \
                            of the agent list. What this rule will not do is pick somebody \
                            else's machine to fill a window with",
            },
        );
        return;
    };

    // Recorded before the ask rather than after: this runs again on the reconcile behind every
    // event, and the machine holds nothing until its answer arrives - so a record written
    // afterwards would ask two or three times over.
    {
        let mut session = poison::lock(&SESSION, "session");
        if !session.tabs_asked_of.insert(daemon.clone()) {
            return;
        }
    }
    ask_for_a_tab(&daemon);
}

/// Asks one machine for a tab, and says what a refusal costs.
///
/// One caller for the empty window and one for a machine holding nothing, so what happens when
/// a daemon says no is written down once.
fn ask_for_a_tab(daemon: &DaemonId) {
    log::info("tab.first.creating", fields! { "daemon" => daemon.to_string() });
    let intent = BackendIntent::CreateTab { tab: mint_tab(), cwd: None, run: None, name: None };
    let asked = submit(daemon, &intent, Keyboard::Follows);
    if let Err(Refusal::Unanswered(detail)) = &asked {
        log::warn(
            "tab.first.unanswered",
            fields! {
                "daemon" => daemon.to_string(),
                "detail" => detail.clone(),
                "impact" => "the daemon may have made the tab, and if it did it arrives on \
                             its own events and this machine fills in. If it did not, this \
                             machine shows nothing in this window, and nothing will ask again - \
                             asking twice could make two",
                "check" => "whether the daemon is keeping up at all: it received the request and \
                            did not answer in time. `muster pane new --daemon <id>` asks by hand \
                            once `muster window` shows the machine still holds nothing",
            },
        );
    } else if let Err(refusal) = asked {
        log::error(
            "tab.first.refused",
            fields! {
                "daemon" => daemon.to_string(),
                "detail" => refusal.to_string(),
                "impact" => "this machine shows nothing in this window and stays that way \
                             until something makes a pane on it. Nothing will ask again - a \
                             rule that retried a refusal would retry it on every event that \
                             arrives from anywhere",
                "check" => "the daemon's own log - it answered its socket, so this is a \
                            refusal rather than an absence. `muster pane new --daemon <id>` \
                            asks once more by hand",
            },
        );
    }
}

/// Whether this machine has told this window what it holds.
///
/// The difference between "this machine is empty" and "this machine has not answered yet", and
/// the reason asking for a tab waits for it. A daemon Muster attached to already has a
/// session, and asking it for a tab in the moment before its first snapshot lands is a
/// window that opens onto a tab nobody asked for and leaves the one they did behind.
///
/// A daemon Muster just started answers this true and holds nothing, which is the case the ask
/// exists for - it simply happens on that bootstrap rather than a moment before it. A daemon
/// still restoring has not said all it holds, and answers false until it says it is done.
fn has_spoken(daemon: &DaemonId) -> bool {
    let session = poison::lock(&SESSION, "session");
    session.backends.get(daemon).is_some_and(|backend| {
        let mirror = poison::lock(&backend.mirror, "mirror");
        mirror.health() == Health::Connected && !mirror.restoring()
    })
}

/// The first attached daemon on this machine, in the order the config named them.
pub(crate) fn first_local_daemon() -> Option<DaemonId> {
    let session = poison::lock(&SESSION, "session");
    session
        .composition
        .daemons()
        .find(|daemon| matches!(daemon.endpoint, Endpoint::Local { .. }))
        .map(|daemon| daemon.id.clone())
}

/// The first daemon this window is attached to at all, local or not.
///
/// The fallback for a request that has to reach some daemon and has no pane to find one
/// from. Kept apart from [`first_local_daemon`] rather than folded into it, because which of
/// the two a caller wants is a decision about whether Muster is acting on its own or on
/// somebody's keystroke, and that is not a decision to make by default.
pub(crate) fn first_attached_daemon() -> Option<DaemonId> {
    let session = poison::lock(&SESSION, "session");
    session.composition.daemons().next().map(|daemon| daemon.id.clone())
}

/// Shows a daemon-owned pane in this window, and points the keyboard at it.
///
/// The daemon is asked where the pane lives before anything is built, because the answer
/// decides whether there is anything to build. A region shows a tab and only the daemon
/// knows which tab a pane is in - and a window that attaches to a pane no daemon holds is
/// the failure that has cost this project the most time, because it looks exactly like a
/// window that renders and ignores the keyboard.
///
/// Which daemon holds the pane is searched for rather than said, because at this moment
/// nobody knows: `argv` carries a pane id and a config file carries daemons, and the two are
/// joined here. Every daemon already being followed is asked; a Muster with no config has one
/// to ask, this install's own daemon on this machine.
pub(crate) fn attach(pane_id: &str) -> Result<Arc<AttachedPane>, AttachError> {
    let pane = PaneId::new(pane_id);
    follow_implicitly_if_nothing_else().map_err(AttachError::Unreachable)?;
    // A window opened onto one pane is a window like any other: it holds tabs, and a tab nobody
    // holds joins it. Without this it never said it was open, so it held nothing but the tab on
    // screen, and every other tab the daemon had was listed by no window at all.
    say_this_window_is_open();

    let (daemon, tab) = locate(&pane).ok_or_else(|| AttachError::NoSuchPane {
        pane: pane_id.to_string(),
        held: panes_followed(),
        dropped: 0,
    })?;

    let attached = {
        let mut session = poison::lock(&SESSION, "session");

        // This pane's channel by hand, before the rest. The reconcile below opens one for
        // every other pane in the tab and logs whatever refuses; this one has a caller
        // waiting on an answer, so its refusal is returned rather than written down.
        session.open_channel(&daemon, &pane).map_err(AttachError::NoChannel)?;

        // One region per tab, not per pane. A tab's panes are the tab's own tree and they
        // are rendered inside one region; attaching a second pane from a tab already on
        // screen is asking for the keyboard, not for a second copy of the tab.
        let region = session
            .composition
            .surface(&daemon, &tab)
            .expect("the daemon holding this pane is one being followed");
        session.composition.focus_pane(region, pane.clone());
        session.reconcile(&daemon);

        session
            .channel(&daemon, &pane)
            .map(Arc::clone)
            .ok_or_else(|| AttachError::NoChannel("the channel opened and then went".to_string()))?
    };

    // What the tabs are is settled by the reconcile above; this is only about which of them
    // this window may show, and about a window somebody asked for that has none.
    mark_opened();
    settle_what_the_window_shows();
    // Outside the lock, because emitting reaches the shell and a shell reacting to an event
    // by dispatching a request is ordinary.
    publish("attach");
    Ok(attached)
}

fn following_anything() -> bool {
    let session = poison::lock(&SESSION, "session");
    !session.backends.is_empty()
}

fn panes_followed() -> usize {
    let session = poison::lock(&SESSION, "session");
    session
        .backends
        .values()
        .map(|backend| poison::lock(&backend.mirror, "mirror"))
        .map(|mirror| mirror.panes().count())
        .sum()
}

/// The directory a pane is sitting in.
///
/// What a new tab beside it starts in. Read here rather than left to the daemon because a new
/// tab has nothing to inherit from, so the daemon would start it in a home directory - and what
/// somebody pressing the key means is "where I already am".
///
/// `None` when the daemon does not hold the pane, or holds it and does not know the directory.
/// The two are one answer on purpose: both mean the request carries no directory and the daemon
/// decides, and a caller that told them apart would have nothing different to do about it.
pub(crate) fn cwd_of(daemon: &DaemonId, pane: &PaneId) -> Option<String> {
    let session = poison::lock(&SESSION, "session");
    let mirror = poison::lock(&session.backends.get(daemon)?.mirror, "mirror");
    let held = mirror.pane(pane)?;
    // An empty directory is the daemon saying it does not know, which is different from a
    // directory somebody chose - and a tab started in "" would be started in `/`.
    (!held.cwd.is_empty()).then(|| held.cwd.clone())
}

/// Which tab a daemon holds this pane in.
///
/// What "rename this tab" means when nobody named a tab: the one holding the pane the keyboard
/// is on. `None` when the daemon does not hold the pane, which is a pane that closed while a
/// keystroke was in flight rather than a state to recover from.
pub(crate) fn tab_of(daemon: &DaemonId, pane: &PaneId) -> Option<TabId> {
    let session = poison::lock(&SESSION, "session");
    let mirror = poison::lock(&session.backends.get(daemon)?.mirror, "mirror");
    Some(mirror.pane(pane)?.tab.clone())
}

/// Which followed daemon holds the tab Muster calls this, if any followed one does.
///
/// What lets a caller name a tab and nothing else, on exactly the terms
/// [`daemon_holding`] gives a pane: a tab name is minted unique across every attached machine,
/// so saying which machine holds it would be asking for something the caller has no way to know.
///
/// The mirror, because it is what the window is actually showing - and a request about a tab
/// that has closed should be refused rather than sent.
pub(crate) fn daemon_holding_tab(tab: &TabId) -> Option<DaemonId> {
    let session = poison::lock(&SESSION, "session");
    session
        .backends
        .iter()
        .find(|(_, backend)| poison::lock(&backend.mirror, "mirror").tab(tab).is_some())
        .map(|(id, _)| id.clone())
}

/// Which followed daemon holds this pane, and where in it.
///
/// A name is Muster's own and unique across every attached machine, so exactly one daemon can
/// hold it. Two would mean one name was minted for two panes, which is a bug in the mint rather
/// than something a caller could have said more precisely - hence the warning rather than a
/// refusal, and the first answer rather than none.
fn locate(pane: &PaneId) -> Option<(DaemonId, TabId)> {
    let session = poison::lock(&SESSION, "session");
    let mut found: Option<(DaemonId, TabId)> = None;
    for (id, backend) in &session.backends {
        let mirror = poison::lock(&backend.mirror, "mirror");
        let Some(held) = mirror.pane(pane) else { continue };
        if let Some((first, ..)) = &found {
            log::warn(
                "pane.ambiguous",
                fields! {
                    "pane" => pane.to_string(),
                    "daemons" => format!("{first}, {id}"),
                    "impact" => "the first of them was used, so a command about this name may \
                                 reach the wrong machine",
                    "check" => "this should be impossible: names are minted unique across \
                                daemons. Look for a saved pane-name file read back under a \
                                different mint, or two Musters writing one",
                },
            );
            break;
        }
        found = Some((id.clone(), held.tab.clone()));
    }
    found
}

/// Settles what this window is showing, after anything that could have changed it.
///
/// Two rules with an order between them. Take in whatever the machines have already said -
/// including any tab nobody holds that is this window's to take - and only then ask for a
/// tab, so the window that asks is one that genuinely has nothing to open onto.
fn settle_what_the_window_shows() {
    reconcile_every_daemon();
    open_a_tab_if_the_window_is_empty();
}

/// Brings every attached machine's tabs into the window, whatever order the events arrived in.
///
/// A launch is two requests - follow the daemons, then open the window - with a renderer, a
/// menu and a window built in between, and a machine's first snapshot lands somewhere in that
/// gap. Reconciling here rather than waiting for the next thing a daemon says is what stops the
/// window deciding it is empty while holding a snapshot that says otherwise, which is a window
/// that asks for a tab nobody wanted and opens onto it.
///
/// Idempotent, and the same call the event path makes for one daemon.
fn reconcile_every_daemon() {
    let mut session = poison::lock(&SESSION, "session");
    let daemons: Vec<DaemonId> = session.backends.keys().cloned().collect();
    for daemon in &daemons {
        session.reconcile(daemon);
    }
}

/// Writes the arrangement down, if it has changed since the last time.
///
/// Called from `publish`, which is every moment composition is settled - and most of those
/// change nothing about it, because a publish also follows every agent transition. So the
/// comparison is against the text last written rather than against the record: the same
/// arrangement renders to the same bytes, and identical bytes are a write that does not
/// happen.
///
/// Replaced rather than appended to, through a temporary beside it: a window that quit while
/// this was half-written would otherwise come back to a file that parses as far as the third
/// region and stops.
fn save(composition: &Composition, presentation: Presentation, font_sizes: &FontSizes) {
    if !opened() {
        return;
    }
    let mut held = poison::lock(&STATE, "saved-arrangement");
    let Some((path, written)) = held.as_mut() else { return };

    let text = saved::to_toml(&Saved::of(composition, presentation, font_sizes));
    if &text == written {
        return;
    }

    let file = PathBuf::from(&*path);
    let staged = file.with_extension("writing");
    let result = std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))
        .and_then(|()| std::fs::write(&staged, &text))
        .and_then(|()| std::fs::rename(&staged, &file));

    match result {
        Ok(()) => *written = text,
        Err(error) => {
            log::warn(
                "composition.save.failed",
                fields! {
                    "path" => path.clone(),
                    "detail" => error.to_string(),
                    "impact" => "this window opens as a first launch does next time - the \
                                 daemons and their panes are unaffected, only the arrangement",
                    "check" => "whether that directory exists and is writable",
                },
            );
            // Cleared so a directory that becomes writable again is picked up by the next
            // publish rather than after the arrangement happens to change twice.
            written.clear();
        }
    }
}

/// The arrangement this window was left in, or nothing.
///
/// A file that will not read is a log line and nothing more. Every way this fails ends with a
/// window that opens the way a first launch does, which is a worse morning and not a broken
/// one - and refusing to open at all over a state file would be the wrong trade by a mile.
fn saved_arrangement() -> Option<Saved> {
    let path = {
        let held = poison::lock(&STATE, "saved-arrangement");
        held.as_ref().map(|(path, _)| path.clone())?
    };
    let text = std::fs::read_to_string(&path).ok()?;
    match saved::from_toml(&text) {
        Ok(saved) => Some(saved),
        Err(detail) => {
            log::warn(
                "composition.restore.failed",
                fields! {
                    "path" => path,
                    "detail" => detail,
                    "impact" => "this window opens as a first launch does; nothing about the \
                                 daemons or their panes is affected",
                    "check" => "the file itself - it is TOML, and it is replaced by the next \
                                arrangement this window settles on",
                },
            );
            None
        }
    }
}

/// The window's own chrome as the file left it, or what a first launch gets.
///
/// Read from disk rather than from the session, because the one caller outside the restore asks
/// during launch: a shell has to know where to put the window before it shows it, and `open()`
/// has not run yet. One function so the two answers cannot differ.
pub(crate) fn saved_presentation() -> Presentation {
    saved_arrangement().map(|saved| saved.presentation).unwrap_or_default()
}

/// Says where the window has settled, so the next launch can put it back.
///
/// One way only. Nothing here announces a `PresentationChanged` in reply, because the shell is
/// the only thing that can move a window and telling it a frame would mean answering a drag
/// that is still happening with where it started.
///
/// Saved rather than published, which is where this differs from every other change here. A
/// frame moves nothing the window is showing, and a drag produces a hundred of these a second -
/// so republishing a view and a roster for each would be per-event work in the one path that
/// cannot afford it. The file is the only thing that has to hear about it.
///
/// A report that arrives before the window has opened is remembered and not written, which is
/// the ordinary case at launch: the shell has a frame the moment the window exists, and the
/// arrangement it would be saved over has not been read yet. `open` writes it a moment later.
pub(crate) fn set_window_frame(frame: Option<Frame>, full_screen: bool) {
    let mut session = poison::lock(&SESSION, "session");
    session.presentation = session.presentation.with_frame(frame, full_screen);
    save(&session.composition, session.presentation, &session.font_sizes);
}

/// Tells the shell what this window is showing.
///
/// The whole view rather than what moved. A shell handed the whole answer holds no picture
/// of its own to patch, and the message is a few hundred bytes for a window nobody can fill
/// past about fifteen panes.
/// `cause` names what asked, for the log line that says the window moved.
fn publish(cause: &str) {
    let publishing = poison::lock(&PUBLISHING, "publishing");
    // What the window is showing is also the answer to which agents have been seen, so the
    // two are settled together rather than left to drift. `noticed` is the panes that were
    // waiting to be noticed and have now been - re-announced below, after the shell has been
    // handed the arrangement they appear in.
    let (view, roster, numbering, noticed, focus, view_message, roster_message) = {
        let mut session = poison::lock(&SESSION, "session");
        // Before the view is built, and over every daemon rather than whichever one prompted
        // this. Several paths change what is on screen without going near a reconcile:
        // showing a tab that was just created, surfacing a pane from the sidebar, giving a
        // daemon its first region, reopening a saved arrangement. Making this a precondition
        // of building the view means none of them can get it wrong, rather than each having
        // to remember - and both halves have already been got wrong that way.
        //
        // A region that has not been reconciled has no pane, so nothing in it has the
        // keyboard and every keybinding meaning "the focused pane" is refused. A pane with no
        // channel is one a shell must not spawn a bridge for, so it renders blank until
        // something republishes.
        let daemons: Vec<DaemonId> = session.backends.keys().cloned().collect();
        for daemon in &daemons {
            session.reconcile(daemon);
        }
        let view = session.view();
        let roster = session.roster(&view);
        let numbering = session.numbering(&roster);
        let noticed = session.attention.showing(view.showing().clone());
        session.report_seen(&noticed.reported);
        let keyboard = session
            .composition
            .focused_region()
            .and_then(|region| Some(PaneKey::new(&region.daemon, region.pane.as_ref()?)));
        let told = session.pane_focus.keyboard(keyboard);
        let focus = session.focus_reports(told);
        // The typeable watch is settled against the same set, and for a reason of its own: what
        // it says is that a pane renders and swallows what is typed into it, and a pane no
        // region is drawing renders nothing - so a socket bound for one is owed no bridge until
        // the window shows it.
        watchdog::showing(view.showing().clone());
        // Here because this is the moment composition is settled, and because everything that
        // changes it ends up here - so nothing has to remember to save.
        save(&session.composition, session.presentation, &session.font_sizes);
        session.forget_what_closed();
        let view_message = convert::view(&view);
        let roster_message = convert::roster(&roster, &numbering);
        let view_message = session.sent.view(&view_message).then_some(view_message);
        let roster_message = session.sent.roster(&roster_message).then_some(roster_message);
        (view, roster, numbering, noticed, focus, view_message, roster_message)
    };

    if view_message.is_none() && roster_message.is_none() {
        log::debug("publish.unchanged", fields! { "cause" => cause });
    }

    // The shape, not the fact. "the view changed" is useless in a bug report and what it
    // changed to is the whole answer - a window rendering the wrong thing and a window
    // rendering nothing are one line apart here (`architecture.md`, the diagnostic log).
    if let Some(message) = view_message {
        for region in &view.regions {
            log::info(
                "view.region",
                fields! {
                    "cause" => cause,
                    "region" => region.id.to_string(),
                    "daemon" => region.daemon.to_string(),
                    "tab" => region.tab.to_string(),
                    "keyboard" => region.pane.as_ref().map(ToString::to_string).unwrap_or_default(),
                    "tree" => match &region.root {
                        Some(root) => root.to_string(),
                        None => "(not yet published)".to_string(),
                    },
                    "focused" => view.focused == Some(region.id),
                },
            );
        }
        ffi::emit(&Event { payload: Some(event::Payload::ViewChanged(message)) });
    }
    if let Some(message) = roster_message {
        log::info(
            "roster.published",
            fields! {
                "cause" => cause,
                "tabs" => roster.tabs().count().to_string(),
                "tabs_on_screen" => roster.tabs().filter(|tab| tab.on_screen).count().to_string(),
                "panes" => roster.panes().count().to_string(),
                "on_screen" => roster.panes().filter(|pane| pane.on_screen).count().to_string(),
                "numbering" => describe_numbering(&numbering),
            },
        );
        ffi::emit(&Event { payload: Some(event::Payload::RosterChanged(message)) });
    }
    drop(publishing);

    tell_focus(focus);
    // After the view, so that a pane surfaced by this very publish has somewhere to be
    // painted before it is told it is no longer waiting on anyone.
    for pane in &noticed.settled {
        announce_state(pane);
    }
    // A pane this publish put on screen is a pane somebody can now see, so whatever it was
    // asking for it is asking no longer.
    for pane in &noticed.withdrawn {
        announce_attention(pane, Attend::Withdrawn);
    }
}

/// Applies what a daemon just said to what Muster is holding open.
///
/// Separate from reporting it, and finished before reporting starts: reporting reaches the
/// shell, the shell reacts by dispatching, and a dispatch that arrived while this held the
/// session would deadlock against it on the same thread.
fn reconcile(daemon: &DaemonId) {
    poison::lock(&SESSION, "session").reconcile(daemon);
    // A standing rule rather than a launch-time one, because the states that produce a
    // daemon with nothing on screen keep arriving: a tab Muster asked for a moment ago
    // and is waiting on, a daemon that came back after a restart with its tabs, a tab closed
    // from another client while its daemon still holds others.
    //
    // Standing from the moment the window knows what it is showing, and not before. Following
    // the daemons and opening the window are two requests with a renderer, a menu and a window
    // built between them, and the first bootstrap arrives in that gap - so without the guard
    // this answers a question `open` is still on its way to answering, and the saved
    // arrangement then lands on top of the answer as a second region onto the same tab.
    // `open` calls this itself, as its own second step, once the restore has had its say.
    //
    // Safe only while nothing closes a region deliberately - the day a user can put one away,
    // this would reopen it on the next thing the daemon said, and the rule needs to learn the
    // difference between empty and dismissed.
    if opened() {
        settle_what_the_window_shows();
    }
}

/// Turns what one daemon said into a log line and, where the window renders it, an event.
///
/// The whole of D's answer to "agent states are the point": every pane's transitions reach
/// the log, and the attached one's reaches the chrome. Which pane the window is showing is
/// the shell's business, so every pane is sent and the shell decides.
fn announce(daemon: &DaemonId, notice: Notice) {
    match notice {
        Notice::Bootstrapped { changes, unreadable } => {
            log::info(
                "mirror.bootstrap",
                fields! {
                    "daemon" => daemon.to_string(),
                    "changes" => changes.len().to_string(),
                    "unreadable" => unreadable.to_string(),
                },
            );
            if unreadable > 0 {
                log::warn(
                    "mirror.tabs_unreadable",
                    fields! {
                        "daemon" => daemon.to_string(),
                        "count" => unreadable.to_string(),
                        "impact" => "these tabs are not drawn, and neither are their panes",
                        "check" => "a daemon of this protocol never sends a tree that does not \
                                    read, so this is a bug on one side or the other - the \
                                    daemon's log for the snapshot, and the protocol versions \
                                    the two sides were built with",
                    },
                );
            }
            // A bootstrap replaces the whole picture, so anything composition names may have
            // gone in the gap it was rebuilt across.
            reconcile(daemon);
            publish("bootstrap");
            health(daemon, Health::Connected, "");
            catch_up_tab_names(daemon);
            for change in changes {
                report(daemon, &change);
            }
        }
        Notice::Changed(change) => {
            // Two questions, not one. Composition is reconciled when something it names may
            // have moved; the view and the roster are republished whenever they would read
            // differently - and a pane's name is in the roster without being anywhere
            // composition can see.
            if change.moves_structure() {
                reconcile(daemon);
            }
            if change.republishes() {
                publish(change.kind());
            }
            // A pane moved into a tab another machine holds makes a nameless part of it here,
            // which takes the tab's name now rather than at this machine's next reconnect.
            if matches!(change, Change::TabAdded(_)) {
                catch_up_tab_names(daemon);
            }
            report(daemon, &change);
        }
        Notice::Stale { detail } => {
            // Deliberately not a detach. A stale daemon is one Muster expects back, and its
            // regions are the last true thing anyone knows about it - closing them would
            // empty the window every time a laptop's lid shut (`architecture.md`,
            // degradation).
            log::warn(
                "backend.stale",
                fields! {
                    "daemon" => daemon.to_string(),
                    "detail" => detail.clone(),
                    "impact" => "panes keep rendering and the agent states shown are now a \
                                 guess about the present",
                    "check" => "whether the daemon is still running, and for a remote one \
                                whether the tunnel is up",
                },
            );
            health(daemon, Health::Stale, &detail);
        }
        Notice::Reconnected => {
            log::info("backend.reconnected", fields! { "daemon" => daemon.to_string() });
            health(daemon, Health::Connected, "");
            report_again(daemon);
        }
    }
}

/// Tells panes they gained or lost the keyboard of a focused window. The daemon writes the
/// report only to a program that asked (`muster_core::pane_focus`).
fn tell_focus(focus: Vec<(Arc<AttachedPane>, bool)>) {
    for (pane, focused) in focus {
        pane.input.focus(focused);
    }
}

/// A daemon came back, so the finishes this window reported seen to it may never have arrived.
/// Attention takes them back and reports again whatever is on screen now; the rest are `done`
/// again until somebody looks (`Attention::reconnected`). A focus report sent while it was away
/// was dropped too, so each of its panes is told again whether it has focus
/// (`PaneFocus::reconnected`).
fn report_again(daemon: &DaemonId) {
    let (noticed, focus) = {
        let mut session = poison::lock(&SESSION, "session");
        let noticed = session.attention.reconnected(daemon);
        session.report_seen(&noticed.reported);
        let panes: Vec<PaneKey> = session.panes.get(daemon).map_or_else(Vec::new, |held| {
            held.keys().map(|pane| PaneKey::new(daemon, pane)).collect()
        });
        let told = session.pane_focus.reconnected(panes);
        (noticed, session.focus_reports(told))
    };
    tell_focus(focus);
    for pane in &noticed.settled {
        announce_state(pane);
    }
}

fn report(daemon: &DaemonId, change: &Change) {
    if let Change::FinishedUnseen { pane, unseen } = change {
        log::info(
            "agent.finished_unseen",
            fields! {
                "daemon" => daemon.to_string(),
                "pane" => pane.to_string(),
                "unseen" => *unseen,
            },
        );
    }
    if let Change::AgentStateChanged { pane, from, to } = change {
        log::info(
            "agent.state",
            fields! {
                "daemon" => daemon.to_string(),
                "pane" => pane.to_string(),
                "from" => from.as_str(),
                "to" => to.as_str(),
            },
        );
    }

    if let Change::Restored(restored) = change {
        restored_from_disk(daemon, restored);
    }
    if let Change::ClipboardWrite { pane, text } = change {
        let allowed = clipboard_writes_allowed();
        log::info(
            "clipboard.write",
            fields! {
                "daemon" => daemon.to_string(),
                "pane" => pane.to_string(),
                "characters" => text.chars().count().to_string(),
                "allowed" => allowed.to_string(),
            },
        );
        if allowed {
            ffi::emit(&Event {
                payload: Some(event::Payload::ClipboardWrite(ClipboardWrite {
                    daemon_id: daemon.to_string(),
                    pane_id: pane.to_string(),
                    text: text.clone(),
                })),
            });
        }
    }
    if let Change::PasteHeld { pane, text } = change {
        log::info(
            "paste.held",
            fields! {
                "daemon" => daemon.to_string(),
                "pane" => pane.to_string(),
                "characters" => text.chars().count().to_string(),
            },
        );
        ffi::emit(&Event {
            payload: Some(event::Payload::PasteHeld(PasteHeld {
                daemon_id: daemon.to_string(),
                pane_id: pane.to_string(),
                text: text.clone(),
            })),
        });
    }

    // Recorded before anything is announced, because it is what the announcement depends on.
    // The lock is let go before anything is emitted, on the same terms as `announce_state`
    // below: emitting reaches the shell, the shell reacts by dispatching, and a dispatch
    // arriving while this held the session would deadlock against it on the same thread.
    let attended = attended(daemon, change);
    if let Some((pane, attend)) = attended {
        announce_attention(&pane, attend);
    }

    if let Some(pane) = change.announces_agent_state() {
        announce_state(&PaneKey::new(daemon, pane));
    }
    if let Change::PaneRemoved(pane) = change {
        watch::publish(&Seen::Closed(PaneKey::new(daemon, pane)));
    }
}

/// A daemon refused this window's report that it saw these panes, so they are `done` here again,
/// as they still are on the daemon (`Attention::refused`).
fn seen_refused(panes: &[PaneKey]) {
    let settled = poison::lock(&SESSION, "session").attention.refused(panes);
    log::warn(
        "attention.seen.refused",
        fields! {
            "panes" => panes.iter().map(ToString::to_string).collect::<Vec<String>>().join(","),
            "impact" => "these panes read done again in this window, as they do on their daemon \
                         and in every other window, until somebody looks at them again",
            "check" => "a daemon refuses changes while it hands its panes to another; its log \
                        says whether that handoff failed, and why",
        },
    );
    for pane in &settled {
        announce_state(pane);
    }
}

/// What a daemon's change does to attention: whether a finish landed on a pane somebody was
/// looking at, which is the difference between `idle` and `done`, and what the pane is now
/// asking of anybody.
///
/// `state_since` is kept beside attention and nowhere else, because these arms are the daemon's
/// own word. A look settling `done` to `idle` goes through attention alone, so it does not
/// restart how long the agent has been resting.
fn attended(daemon: &DaemonId, change: &Change) -> Option<(PaneKey, Attend)> {
    match change {
        Change::AgentStateChanged { pane, from, to } => {
            let key = PaneKey::new(daemon, pane);
            let mut session = poison::lock(&SESSION, "session");
            if from != to {
                session.state_since.insert(key.clone(), clock::wall_clock_millis());
            }
            session.observe(&key).map(|attend| (key, attend))
        }
        Change::FinishedUnseen { pane, .. } => {
            let key = PaneKey::new(daemon, pane);
            let mut session = poison::lock(&SESSION, "session");
            session.observe(&key).map(|attend| (key, attend))
        }
        // A pane this window is meeting for the first time. Muster saw no transition for it,
        // so nothing is asked of anybody: a banner would be Muster announcing history at launch.
        Change::PaneAdded(pane) => {
            let key = PaneKey::new(daemon, pane);
            let mut session = poison::lock(&SESSION, "session");
            if session.agent_state(&key).is_some() {
                session.state_since.entry(key.clone()).or_insert_with(clock::wall_clock_millis);
            }
            let finished = session.finished_unseen(&key);
            if session.attention.met(&key, finished) {
                session.report_seen(std::slice::from_ref(&key));
            }
            None
        }
        Change::PaneRemoved(pane) => {
            let key = PaneKey::new(daemon, pane);
            let mut session = poison::lock(&SESSION, "session");
            session.state_since.remove(&key);
            session.pane_focus.forget(&key);
            let attended = session.attention.forget(&key);
            attended.map(|attend| (key, attend))
        }
        _ => None,
    }
}

/// What a daemon said when it finished putting back what it held before it last stopped.
///
/// Nothing asks a restoring daemon for a first tab, because its tabs are on their way; so this
/// is where that ask happens, if the window is still empty now that they have arrived.
fn restored_from_disk(daemon: &DaemonId, restored: &Restored) {
    log::info(
        "daemon.restored",
        fields! {
            "daemon" => daemon.to_string(),
            "lost_tabs" => restored.lost_tabs.len().to_string(),
            "lost_panes" => restored.lost_panes.len().to_string(),
        },
    );
    if restored.saving_stopped {
        log::warn(
            "daemon.saving_stopped",
            fields! {
                "daemon" => daemon.to_string(),
                "impact" => "the daemon has stopped writing down its tabs and panes, so if it \
                             stops, what changed from now on does not come back",
                "check" => "the daemon's own log, which says why it could not save - most \
                            often a full or read-only disk",
            },
        );
    }
    if opened() {
        settle_what_the_window_shows();
    }
}

/// Renames this machine's part of each grouped tab whose name is behind another part's.
///
/// A tab spanning machines holds its name on each of them, ordered by a generation, and a
/// machine that was away when the tab was renamed comes back with the old name (MIP-3, section
/// 2). The part with the highest generation holds the name; a lagging part is told it, at that
/// generation, when its machine answers again, and a part a move just made is told it at once.
fn catch_up_tab_names(daemon: &DaemonId) {
    let lagging: Vec<(TabId, Option<String>, u64)> = {
        let session = poison::lock(&SESSION, "session");
        let Some(backend) = session.backends.get(daemon) else { return };
        let here: Vec<(TabId, u64)> = poison::lock(&backend.mirror, "mirror")
            .tabs()
            .map(|tab| (tab.id.clone(), tab.generation))
            .collect();
        here.into_iter()
            .filter_map(|(tab, generation)| {
                let (newest, label) = session
                    .backends
                    .iter()
                    .filter(|(other, _)| *other != daemon)
                    .filter_map(|(_, other)| {
                        let mirror = poison::lock(&other.mirror, "mirror");
                        mirror.tab(&tab).map(|part| (part.generation, part.label.clone()))
                    })
                    .max_by_key(|(generation, _)| *generation)?;
                (newest > generation).then_some((tab, label, newest))
            })
            .collect()
    };
    for (tab, name, generation) in lagging {
        let intent = BackendIntent::RenameTab { tab: tab.clone(), name, generation };
        if let Err(refusal) = submit(daemon, &intent, Keyboard::StaysPut) {
            log::warn(
                "tab.name.catch_up_failed",
                fields! {
                    "daemon" => daemon.to_string(),
                    "tab" => tab.to_string(),
                    "detail" => refusal.to_string(),
                    "impact" => "this machine's part of the tab keeps an older name; the \
                                 window shows the newest one, and another window reading only \
                                 this machine shows the old one",
                    "check" => "`muster tab rename` sets the name on every part again",
                },
            );
        }
    }
}

/// Tells the shell that a pane has started asking for somebody, or stopped.
///
/// The label travels with it rather than being looked up by the shell, because naming a pane
/// is the core's decision (`roster`) and a banner naming an agent differently from the row it
/// appears on is two names for one thing.
fn announce_attention(pane: &PaneKey, attend: Attend) {
    if matches!(attend, Attend::Raised(_)) && !speaks_for(pane) {
        return;
    }
    let (state, label, subtitle) = match attend {
        Attend::Raised(alert) => {
            let (label, subtitle) = describe_pane(pane).unwrap_or_default();
            (alert.as_str().to_string(), label, subtitle)
        }
        // Nothing to describe for a withdrawal, and often nothing left to describe it from -
        // the commonest one is a pane that closed.
        Attend::Withdrawn => (String::new(), String::new(), String::new()),
    };
    log::info(
        "attention.changed",
        fields! {
            "daemon" => pane.daemon.to_string(),
            "pane" => pane.pane.to_string(),
            "state" => if state.is_empty() { "(withdrawn)".to_string() } else { state.clone() },
        },
    );
    ffi::emit(&Event {
        payload: Some(event::Payload::AttentionChanged(AttentionChanged {
            daemon_id: pane.daemon.to_string(),
            pane_id: pane.pane.to_string(),
            state,
            label,
            subtitle,
        })),
    });
}

/// Whether this window is the one to tell somebody a pane needs them.
///
/// A window speaks for its own tabs. Another open window speaks for its own, so two windows never
/// post one agent twice. A closed window cannot speak at all, and its agents are still running -
/// so the open window that came to the front most recently speaks for it, and clicking what it
/// posts reopens that window onto the tab (kan a_2Mhi0EZlv). A tab nobody holds yet is the same
/// case: every window hears its agents, and only the one in front says so.
///
/// Asked only when a pane starts asking for somebody, which is rare next to everything else a
/// window hears - so dialing the other windows here costs nothing anybody will notice.
fn speaks_for(pane: &PaneKey) -> bool {
    let Some(tab) = tab_of_pane(&pane.pane) else { return true };
    let (me, holders) = {
        let session = poison::lock(&SESSION, "session");
        if session.holding.holds(&tab) || !session.holding.is_shared() {
            return true;
        }
        (session.holding.me().clone(), session.holding.holders().clone())
    };
    let open = |window: &HeldWindow| crate::holding::is_open(&me, window);
    if let Some(holder) = holders.holder(&tab).and_then(|name| holders.window(name))
        && open(holder)
    {
        return false;
    }
    holders.in_front(&pane.daemon, open) == Some(&me)
}

/// What to call one pane, and what its agent says it is doing.
///
/// One mirror rather than a whole roster. `Roster::of` locks every attached daemon and walks
/// every pane in the window, which is right for a list and far more than a single banner
/// needs - but it is the same two decisions, taken from the same two functions, so the name
/// on a notification and the name on its row cannot come apart.
fn describe_pane(pane: &PaneKey) -> Option<(String, String)> {
    let session = poison::lock(&SESSION, "session");
    let mirror = poison::lock(&session.backends.get(&pane.daemon)?.mirror, "mirror");
    let held = mirror.pane(&pane.pane)?;
    let label = muster_core::roster::pane_label(held);
    let subtitle = muster_core::roster::pane_subtitle(held, &label).unwrap_or_default();
    Some((label, subtitle))
}

/// Tells the shell what to paint for one pane's agent.
fn announce_state(pane: &PaneKey) {
    // Resolved before emitting, and with the lock let go in between. Emitting reaches the
    // shell, the shell reacts by dispatching, and a dispatch arriving while this held the
    // session would deadlock against it on the same thread.
    let Some(agent) = presented(pane) else { return };
    ffi::emit(&Event {
        payload: Some(event::Payload::PaneStateChanged(convert::pane_state(&agent))),
    });
    watch::publish(&Seen::State(agent));
}

/// What the window should show for a pane, which is not always what the daemon said.
///
/// The mirror is read back rather than the change being taken at its word, because one of
/// the two changes that announce a pane carries no state at all - a pane that appears
/// already running is the case that needs this. For a transition the mirror was written
/// before this runs, so it holds exactly what the transition moved to.
///
/// Then `done` is laid over it: a finish the daemon holds that this window has not yet
/// reported seen (`attention`).
fn presented(pane: &PaneKey) -> Option<PaneAgent> {
    let session = poison::lock(&SESSION, "session");
    let state = session.presented_state(pane)?;
    Some(session.presented(pane, state))
}

/// Shows the roster or puts it away, and says what it settled on.
///
/// The write goes through `publish` like every other change, which is what gets it saved: the
/// arrangement is written down at the one moment composition is settled, and adding a second
/// place that remembers to save would be a second place that can forget.
pub(crate) fn toggle_sidebar() {
    let shown = !poison::lock(&SESSION, "session").presentation.sidebar;
    // Whatever Muster decided about the roster, the person just decided otherwise. Forgetting
    // that we opened it is what stops the last error clearing later and closing a roster
    // somebody deliberately reopened - or reopening one they deliberately put away.
    poison::lock(&PROBLEMS, "problems").get_or_insert_with(ProblemState::default).opened_sidebar =
        false;
    log::info("presentation.sidebar", fields! { "shown" => shown });
    set_sidebar(shown);
}

/// Types text into a pane by name, whether or not a region is showing it.
///
/// Through the daemon's input connection, which never blocks and answers nothing: the daemon
/// writes it as a paste it never holds, then presses Return if asked. Whether it arrived is
/// what a caller's read-back is for.
pub(crate) fn send_to_pane(
    daemon: &DaemonId,
    pane: &PaneId,
    text: String,
    enter: bool,
) -> Result<(), Refusal> {
    let input = {
        let session = poison::lock(&SESSION, "session");
        let backend = session.backends.get(daemon).ok_or_else(|| {
            Refusal::Declined(format!(
                "this window is not following a daemon called {daemon}, so nothing was sent."
            ))
        })?;
        if poison::lock(&backend.mirror, "mirror").pane(pane).is_none() {
            return Err(Refusal::NotThere(format!(
                "{daemon} holds no pane called {pane}, so nothing was sent. Most likely it \
                 closed while this was in flight."
            )));
        }
        Arc::clone(&backend.input)
    };
    input.send(pane, InputEvent::Send { text, enter }).map_err(|not_sent| {
        Refusal::Declined(format!(
            "nothing was sent to {pane}: {not_sent}. The window reconnects to a daemon on its \
             own; `muster window` says whether {daemon} is connected, and sending again once it \
             is sends it once."
        ))
    })
}

/// Reads a pane back, and changes nothing.
///
/// A round trip at the moment somebody asks: a pane's output never enters the core, so there is
/// nothing held here to answer from. The
/// channel is taken and the lock dropped before the request goes, because a read is a round
/// trip and holding the session across one stalls every event arriving from every other
/// daemon.
pub(crate) fn read_pane(daemon: &DaemonId, pane: &PaneId, rows: u32) -> Result<PaneText, String> {
    let channel = channel(daemon)?;
    channel
        .read(pane)
        // Asked for whole and cut here, so `rows` is the count `docs/cli/agents.md` promises
        // rather than a ceiling on grid rows that a quiet pane spends on blanks.
        .map(|read| read.tail(rows))
        .map_err(|refusal| format!("the daemon {daemon} would not read pane {pane}: {refusal}"))
}

/// Every daemon Muster has started on this machine, checked, and marked where this window is
/// using one.
///
/// `None` means the shell named nowhere to write records, which is a different answer from an
/// empty list: one says nothing was remembered and the other says nothing is there.
///
/// `attached_here` is the only part a window can add that the record cannot, and it is the part
/// that decides whether ending a daemon costs this window anything. A window can only speak for
/// itself, which is why the field is named for that rather than for "in use".
pub(crate) fn daemon_census() -> Option<Vec<(records::Census, bool)>> {
    let directory = daemon_records_path()?;
    let attached: BTreeSet<String> = poison::lock(&SESSION, "session")
        .backends
        .values()
        .map(|backend| backend.socket_path.clone())
        .collect();
    Some(
        records::census(&directory)
            .into_iter()
            .map(|found| {
                let here = attached.contains(&found.socket);
                (found, here)
            })
            .collect(),
    )
}

/// The way to one daemon, or why there is not one.
fn channel(daemon: &DaemonId) -> Result<Arc<dyn BackendChannel>, String> {
    poison::lock(&SESSION, "session").channel_of(daemon).ok_or_else(|| {
        format!(
            "the daemon {daemon} is in this window's composition and is not being followed, \
                 which is a bug in the core rather than a state to recover from"
        )
    })
}

/// The `[[daemon]]` blocks the running configuration was built from.
///
/// Held separately from what is attached, because those are different questions and only one of
/// them is about the file. A config naming no daemons still ends up with one attached - Muster
/// starts its own when nothing answers - so comparing a new file against what is attached would
/// report a change on every reload of a file that never mentioned a daemon at all.
static CONFIGURED_DAEMONS: Mutex<Option<Vec<Daemon>>> = Mutex::new(None);

pub(crate) fn set_configured_daemons(daemons: &[Daemon]) {
    *poison::lock(&CONFIGURED_DAEMONS, "settings") = Some(daemons.to_vec());
}

/// Whether a file names a different set of daemons from the one this window was built from.
///
/// The one thing a reload does not act on, so it is the one thing worth asking about: a
/// `[[daemon]]` change is a question about live sessions rather than about settings, and
/// applying it would move panes somebody is working in.
///
/// Compared by what a person wrote rather than by what came of it - a daemon that is named and
/// failed to attach is not a difference, it is the same wish and the same disappointment.
pub(crate) fn daemons_differ(config: &Config) -> bool {
    let configured = poison::lock(&CONFIGURED_DAEMONS, "settings");
    configured.as_deref().unwrap_or_default() != config.daemons.as_slice()
}

/// Points every attached pane at typing settings that have just been read again.
///
/// Every pane or none, which is the whole reason this exists rather than only setting the
/// static: a reload that reached the static alone would take effect on panes opened afterwards
/// and leave the rest as they were, so what `option_as_alt` meant would depend on when each
/// pane happened to be opened.
///
pub(crate) fn reset_pane_input(settings: &PaneInputSettings) {
    set_pane_input(settings.clone());

    let panes: Vec<Arc<AttachedPane>> = {
        let session = poison::lock(&SESSION, "session");
        session.panes.values().flat_map(|panes| panes.values().map(Arc::clone)).collect()
    };
    for held in &panes {
        held.input.resettle(settings);
    }
    log::info("config.reload.typing", fields! { "panes" => panes.len().to_string() });
}

/// One press of a font-size chord, on the pane the keyboard is on.
///
/// The offset is saturated by the setter rather than refused there. Somebody holding the key
/// down is asking to keep going, and the honest answer at the end of the range is text that
/// stops growing - not a refusal for a keystroke they cannot see the result of anyway.
///
/// Nothing is announced on its own. The size rides on the pane in the view, so the publish
/// below is what tells the shell - and it is the same publish that would have told it about
/// the pane appearing in the first place.
pub(crate) fn adjust_font_size(change: FontSizeChange) -> Result<(), String> {
    let (pane, offset) = {
        let mut session = poison::lock(&SESSION, "session");
        let Some(region) = session.composition.focused_region() else {
            return Err(no_pane_to_size());
        };
        let Some(pane) = region.pane.clone() else { return Err(no_pane_to_size()) };
        let pane = PaneKey::new(&region.daemon, &pane);
        let offset = session.font_sizes.adjust(&pane, change);
        (pane, offset)
    };
    log::info(
        "pane.font_size",
        fields! { "pane" => pane.to_string(), "offset" => offset.to_string() },
    );
    publish("font_size");
    Ok(())
}

fn no_pane_to_size() -> String {
    "no pane has this window's keyboard, so there was no text to size. Text size is per pane \
     now, and this chord means the pane in front of you - the attach failed earlier, or the \
     pane it succeeded on exited."
        .to_string()
}

/// The shell's word that this process is going away, acted on before its bridges are killed.
///
/// The other windows hear it first, even if ending the sessions runs long. This window keeps
/// its tabs: its agents are still running, and reopening it comes back to them - unless ending
/// them is what was asked.
pub(crate) fn quitting(close_sessions: bool) {
    poison::lock(&SESSION, "session").holding.close();
    if !close_sessions {
        return;
    }
    let daemons: Vec<(DaemonId, String)> = {
        let session = poison::lock(&SESSION, "session");
        session
            .backends
            .iter()
            .map(|(id, backend)| (id.clone(), backend.socket_path.clone()))
            .collect()
    };
    close_daemons(&daemons);
}

/// Asks every daemon this window is attached to stop, and says what came of it.
///
/// A stop request rather than a signal, because a signal is not available: Muster puts a daemon
/// it starts in a session of its own so that quitting cannot take the agents with it, and a
/// daemon on another machine has no pid here at all. The socket is the only handle, and it is
/// the better one - it gives a pane's process a catchable SIGHUP and a moment to act in rather
/// than a SIGKILL (kan a_28YghIUw2).
///
/// A longer timeout than an ordinary request. This is a daemon tearing down every pane it
/// holds, and the alternative to waiting is reporting a failure for a stop that worked.
///
/// A daemon that will not answer is reported and left. It is still running, its agents are
/// still going, and the honest thing is to say which one rather than to insist - `muster
/// window` names every daemon and its socket, which is what makes ending one by hand safe.
fn close_daemons(daemons: &[(DaemonId, String)]) {
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);
    let (mut stopped, mut refused) = (0usize, Vec::new());
    for (daemon, socket_path) in daemons {
        match launch::stop(socket_path.as_ref(), PATIENCE) {
            Ok(()) => stopped += 1,
            Err(failure) => {
                refused.push(daemon.to_string());
                log::warn(
                    "quit.session.kept",
                    fields! {
                        "daemon" => daemon.to_string(),
                        "socket" => socket_path.clone(),
                        "detail" => failure,
                        "impact" => "this machine's session is still running with its agents \
                                     in it, although quitting was asked to end it",
                        "check" => "whether that daemon is answering at all - `muster daemons` \
                                    names its socket, and `lsof <socket>` the process holding \
                                    it, which ends it by hand",
                    },
                );
            }
        }
    }
    log::info(
        "quit.sessions.closed",
        fields! {
            "stopped" => stopped.to_string(),
            "kept" => refused.len().to_string(),
            "refused" => refused.join(","),
        },
    );
}

/// Tells the shell what the window should be showing of itself.
///
/// Sent whole and sent on startup as well as on every change, so a shell holds no default of
/// its own. A shell that guessed would be a second answer to a question the core owns, and
/// the two would disagree the first time the default moved.
fn announce_presentation(presentation: Presentation) {
    ffi::emit(&Event {
        payload: Some(event::Payload::PresentationChanged(PresentationChanged {
            sidebar: presentation.sidebar,
        })),
    });
}

/// Makes this window match the record of which window holds each tab, after something changed it.
///
/// A tab another window has taken leaves this window's list, and its surfaces go with it - so its
/// terminals are free for the window that has it now, and the panes in it keep running. A tab
/// given to this window joins the end of the list at the reconcile below, and is not brought on
/// screen: a window that switched tabs because of something done elsewhere is one that types
/// into the wrong pane.
pub(crate) fn follow_the_record() {
    {
        let mut session = poison::lock(&SESSION, "session");
        if !session.holding.reread() {
            return;
        }
        let lost: Vec<TabId> =
            session.composition.held().filter(|tab| !session.holding.holds(tab)).cloned().collect();
        for tab in &lost {
            session.composition.let_go(tab);
        }
        if !lost.is_empty() {
            log::info(
                "holding.lost",
                fields! {
                    "tabs" => lost.iter().map(TabId::as_str).collect::<Vec<&str>>().join(","),
                },
            );
        }
    }
    reconcile_every_daemon();
    publish("holders");
}

/// The window gained or lost the OS's focus.
///
/// The one thing about attention no daemon can tell the core and no core can observe. What
/// it changes is which finished agents this window has seen, so only those panes are
/// re-announced and reported to their daemons - an agent-state change costs that change
/// rather than a walk of every pane (`architecture.md`, fast is a feature).
pub(crate) fn window_focused(focused: bool) {
    log::info("window.focus", fields! { "focused" => focused });
    let (noticed, focus) = {
        let mut session = poison::lock(&SESSION, "session");
        // Coming to the front is what makes this the window a tab nobody holds joins.
        if focused {
            session.holding.focused();
        }
        let noticed = session.attention.window_focused(focused);
        session.report_seen(&noticed.reported);
        let told = session.pane_focus.window_focused(focused);
        (noticed, session.focus_reports(told))
    };
    tell_focus(focus);
    for pane in &noticed.settled {
        announce_state(pane);
    }
    // A pane somebody has just looked at is not asking any more, whatever its state. That is
    // the same rule the border already follows and the reason a focused window showing a pane
    // never raised one in the first place.
    for pane in &noticed.withdrawn {
        announce_attention(pane, Attend::Withdrawn);
    }
    if focused && take_what_nobody_holds() {
        publish("holders");
    }
}

/// How much of one daemon's truth the core currently has.
///
/// Per daemon, because health is per connection. A window showing a laptop and a devenv has
/// two answers, and one of them going stale says nothing about the other - so a single
/// window-wide state would let a dropped VPN read as though every session had gone.
fn health(daemon: &DaemonId, health: Health, detail: &str) {
    // Here rather than at each call site, so that every path which tells the shell a machine
    // has gone tells the watchdog and every open watch too. A stale daemon takes its panes'
    // output with it and says so once, naming the machine; eight more rows saying each of its
    // panes stopped painting would bury the one that names the cause.
    watchdog::daemon_away(daemon, health == Health::Stale);
    let heard = DaemonHealth { daemon: daemon.clone(), health, detail: detail.to_string() };
    ffi::emit(&Event {
        payload: Some(event::Payload::BackendHealth(convert::backend_health(&heard))),
    });
    watch::publish(&Seen::Health(heard));
}

/// The moment the pane becomes typeable: its bridge said `attached`, heard on the thread that
/// reads the bridge's link.
fn typeable(daemon: &DaemonId, pane: &PaneId) {
    let key = PaneKey::new(daemon, pane);
    // Something is painting this pane again, so the next bridge to stop is news.
    poison::lock(&DARK, "dark-panes").remove(&key);
    watchdog::typeable(&key);
    ffi::emit(&Event {
        payload: Some(event::Payload::PaneTypeable(PaneTypeable {
            daemon_id: daemon.to_string(),
            pane_id: pane.to_string(),
        })),
    });
}
