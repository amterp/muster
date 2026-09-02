//! What this process is holding open: daemons, their mirrors, and the attached panes.
//!
//! The runtime half of composition. Which daemons are attached and what each region shows
//! is a record in the core, judged by `composition.json` with no socket in sight; what is
//! here is the part that genuinely needs one - a held-open subscription per daemon, a bound
//! socket per pane, and the threads behind both.
//!
//! Keyed by daemon and by pane throughout, because both are plural. One window can show a
//! laptop and a devenv side by side, and two daemons hand out the same pane ids - `w1:p1`
//! means something on each - so a map keyed by pane alone would let one daemon's pane
//! answer for another's.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use muster_core::AgentState;
use muster_core::attention::{Attend, Attention, Notifications};
use muster_core::composition::{
    Composition, Daemon, DaemonId, Endpoint, FontSizeChange, FontSizes, Frame, PaneKey,
    Presentation, RegionId, Saved, Step, TabKey, Transport, View, ViewPane, saved,
};
use muster_core::config::{Appearance, Config, Feel, Panes};
use muster_core::diagnostics::{clock, log, poison};
use muster_core::fields;
use muster_core::find::{Found, Needle};
use muster_core::input::{Bindings, PaneInput, PaneInputSettings, ScrollDirection};
use muster_core::intent::{BackendChannel, BackendIntent, MoveDestination, Refusal};
use muster_core::mirror::backend::{PaneId, PaneText, Snapshot, TabId};
use muster_core::mirror::{Change, Health, Mirror};
use muster_core::names::{self, Mint, Names, PaneNames, TabNames};
use muster_core::problems::{Problem, Problems, Severity};
use muster_core::reconnect;
use muster_core::respawn::{self, Decision, Ended, Ending, Respawns};
use muster_core::roster::{Numbering, Roster, RosterTab, TabStep};
use muster_herdr::subscription::{Notice, Subscription};
use muster_herdr::{
    HerdrBackend, HerdrClient, HerdrPaneChannel, PaneControlChannel, PaneEnvironment, Reports,
    daemon, fetch_snapshot, own_socket_path, remote,
};
use muster_ssh::{Forward, Remote, State as TunnelState, Tunnel, remote_environment};
use muster_vt::KeyEncoder;

use crate::proto::{
    AttentionChanged, BackendHealth, Event, PaneStateChanged, PaneTypeable, PresentationChanged,
    Problem as ProblemMessage, ProblemsChanged, event,
};
use crate::shared_names::NamesFile;
use crate::{command, convert, ffi, watchdog};

/// What Muster calls the daemon it found for itself.
///
/// The name for the one nobody named. A config file that lists daemons names its own, and
/// this is what a config-less Muster calls the herdr on this machine.
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

/// Where Muster keeps what it downloaded, for the one thing that downloads anything.
///
/// Held on the same terms as the three above, and needed in the same place: attaching a daemon on
/// another machine means having that machine's herdr to hand, and the pinned release asset is
/// fetched here rather than over there.
///
/// None means the shell found nowhere, and then a fetch goes to a temporary that is thrown away -
/// slow on every launch that has to install, rather than broken.
static CACHE: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_cache_path(path: &str) {
    let mut held = poison::lock(&CACHE, "cache");
    *held = if path.is_empty() { None } else { Some(path.to_string()) };
}

fn cache_path() -> Option<String> {
    poison::lock(&CACHE, "cache").clone()
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

/// Where the name registries are written, and the text last written there.
///
/// Beside [`STATE`] and for the same reasons, including the string compare: the names change
/// only when a pane or a tab appears or goes, and a publish follows every agent transition.
static NAMES_FILE: Mutex<Option<(String, String)>> = Mutex::new(None);

/// That same file as the record two Musters name things in, rather than as somewhere to save.
///
/// Kept apart from [`NAMES_FILE`] above because they answer different questions: that one is
/// "has anything changed since the last write", which is a cache and belongs to this process,
/// and this one is the hold every naming goes through, which belongs to whoever else is open.
static SHARED_NAMES: Mutex<Option<Arc<NamesFile>>> = Mutex::new(None);

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

/// What a pane should be, held for the daemon that is about to be started.
///
/// Beside [`FEEL`] and read in one place: the moment a local daemon is reached, which is
/// where the derived config file has to exist before a spawn that takes no arguments.
static PANES: Mutex<Option<Panes>> = Mutex::new(None);

pub(crate) fn set_panes(panes: Panes) {
    *poison::lock(&PANES, "pane-settings") = Some(panes);
}

/// What a pane should be, which with no config file is whatever the daemon would have done -
/// except for the update checks, which Muster turns off either way.
fn panes() -> Panes {
    poison::lock(&PANES, "pane-settings").clone().unwrap_or_default()
}

/// Where Muster writes the config file its daemon reads, and what it last wrote there.
///
/// The same shape as [`STATE`] and for the same reason: the file is rewritten whenever the
/// config file is, most rewrites say exactly what the last one did, and a daemon has nothing
/// to re-read when the text has not moved.
///
/// None means the shell named nowhere to write one, which is what every seam test that sets
/// no path gets. The daemon then reads the user's own herdr config, as it did before this
/// existed, and the run log says so.
static DAEMON_CONFIG: Mutex<Option<(String, String)>> = Mutex::new(None);

pub(crate) fn set_daemon_config_path(path: &str) {
    let mut held = poison::lock(&DAEMON_CONFIG, "daemon-config");
    *held = if path.is_empty() { None } else { Some((path.to_string(), String::new())) };
}

/// The root knobs, held for whatever asks about them next.
///
/// Beside [`BINDINGS`] and [`PANE_INPUT`], for the same reason: a resize arrives from a
/// keystroke and a scroll from a wheel, and neither caller has a config file in hand.
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
/// Two things watch a bridge die, and either may arrive first: the control socket this window
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
    reconcile_sidebar_with_problems();
    announce_problems();
}

/// Records that something is no longer wrong.
///
/// Called from every success path, including the ones where nothing was ever wrong, so the
/// common case is a call that changes nothing and says nothing.
pub(crate) fn clear_problem(key: &str) {
    let changed = {
        let mut held = poison::lock(&PROBLEMS, "problems");
        held.get_or_insert_with(ProblemState::default).problems.clear(key)
    };
    if !changed {
        return;
    }
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
    publish();
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

/// Says that somebody asked for this window, rather than it being the one Muster comes back to.
///
/// On the session rather than in a static beside the paths above, because it is not a setting:
/// it describes this launch, and a test that reset the statics and not this one would open its
/// window under the last test's answer.
pub(crate) fn set_fresh(fresh: bool) {
    poison::lock(&SESSION, "session").fresh = fresh;
}

/// Says where names are remembered, and reads back the ones already there.
///
/// Read here rather than lazily, because it has to happen before any daemon is attached: the
/// first snapshot mints a name for every pane and tab it describes, and a name minted for
/// something that already had one is a pane whose environment now names something else, or a
/// tab the saved arrangement can no longer find.
///
/// The same path becomes the record this window shares with any other Muster that is open, and
/// from here on nothing writes it except through that - see `shared_names`.
pub(crate) fn set_pane_names_path(path: &str) {
    *poison::lock(&NAMES_FILE, "saved-names") =
        if path.is_empty() { None } else { Some((path.to_string(), String::new())) };
    *poison::lock(&SHARED_NAMES, "shared-names") =
        (!path.is_empty()).then(|| Arc::new(NamesFile::at(path)));
    if path.is_empty() {
        return;
    }

    let Ok(text) = std::fs::read_to_string(path) else { return };
    match names::from_toml(&text, Mint::Drawn) {
        Ok((panes, tabs)) => {
            let session = poison::lock(&SESSION, "session");
            *poison::lock(&session.names, "pane-names") = panes;
            *poison::lock(&session.tab_names, "tab-names") = tabs;
        }
        Err(detail) => log::warn(
            "names.restore.failed",
            fields! {
                "path" => path.to_string(),
                "detail" => detail,
                "impact" => "every pane already open is named again, so a program still \
                             running in one holds a name that resolves to nothing and its \
                             commands are refused; and every tab is named again, so this \
                             window opens without the arrangement it was left in",
                "check" => "the file - it is TOML, and it is replaced by the next publish. A \
                            pane made from now on is named and told its name as usual",
            },
        ),
    }
}

/// A daemon's endpoint, turned into something that can be connected to.
///
/// The one place local and remote differ. Everything past this point holds a socket path and
/// never asks where it goes, which is the property that lets one adapter serve both.
#[derive(Debug)]
struct Reached {
    socket_path: String,
    tunnel: Option<Tunnel>,
    /// What every pane made on this daemon is handed.
    ///
    /// A restore where Muster redirected the daemon's config, and nothing where it did not:
    /// a daemon somebody named by `socket` and a daemon on another machine were pointed
    /// somewhere by nobody, so a pane on either already reads what it always did.
    panes: PaneEnvironment,
    /// Whether Muster wrote this daemon's config, which is a different question from whether
    /// a pane needs anything put back - and stays different once a pane carries more than the
    /// one restored variable.
    owns_config: bool,
    /// Where that config went, when it went to another machine.
    ///
    /// None for a daemon on this one, whose file is the single path the shell named. A remote
    /// daemon's is on the far side, so a setting changed while the window is open has to be
    /// sent again before that daemon is asked to read it - and this is the only record of
    /// where to send it.
    remote_config: Option<String>,
    /// Whether Muster started this daemon, as against attaching to one that was answering.
    ///
    /// The distinction nothing recorded, and the one that makes a lifecycle decision safe.
    /// `daemon::ensure_running` attaches to whatever answers on the socket, so a Muster
    /// launched today can adopt a daemon started eighteen hours ago holding somebody's working
    /// agent - which happened, and which reads in `ps` exactly like the scratch daemons beside
    /// it (kan a_28YghIUw2). "The daemons Muster started" and "the daemons Muster is using" are
    /// different sets, and only the second is knowable without this.
    started: bool,
}

/// Writes the config Muster's daemon reads, and says where it went and whether it moved.
///
/// None when the shell named nowhere to write one. The daemon then reads the user's own herdr
/// config, which is what it did before this existed - so the fallback is the old behaviour
/// rather than a broken one, and the run log names which file is in play either way.
///
/// The flag is whether the text changed since the last write. It is the whole difference
/// between a daemon that has something to re-read and one that does not: herdr reads its
/// config at startup and on request, so a file that says what it already said is a request
/// nobody needs to make.
/// The config Muster derives from its own, as the bytes a daemon reads.
///
/// One text, whichever machine the daemon is on: a person writes `scrollback_bytes` once and
/// every daemon Muster starts runs it. What differs is where the file lands - a path the shell
/// named, here, and a path under the far machine's own home, over there.
fn daemon_configuration_text() -> String {
    muster_herdr::configuration_text(&panes())
}

fn write_daemon_configuration() -> Option<(String, bool)> {
    let mut held = poison::lock(&DAEMON_CONFIG, "daemon-config");
    let (path, written) = held.as_mut()?;

    match muster_herdr::write_configuration(path, &panes()) {
        Ok(text) => {
            let changed = &text != written;
            if changed {
                log::info(
                    "daemon.config.written",
                    fields! {
                        "path" => path.clone(),
                        "impact" => "a daemon started from here runs these settings; one that \
                                     was already running keeps what it was started with until \
                                     it is asked to read this again",
                    },
                );
                *written = text;
            }
            Some((path.clone(), changed))
        }
        Err(detail) => {
            log::warn(
                "daemon.config.failed",
                fields! {
                    "path" => path.clone(),
                    "detail" => detail,
                    "impact" => "the daemon falls back to the user's own herdr config, so what \
                                 a pane runs and how deep its scrollback is come from a file \
                                 Muster did not write - and its update checks are back on, \
                                 which can move the pinned daemon off its pin",
                    "check" => "whether that directory exists and is writable",
                },
            );
            // Cleared so a directory that becomes writable again is picked up by the next
            // write rather than after the settings happen to change twice.
            written.clear();
            None
        }
    }
}

/// Opens whatever a daemon's endpoint describes.
///
/// For a daemon on this machine that is a path, found the way herdr's own client finds it
/// when the config did not say. For a remote one it is an ssh master forwarding that
/// daemon's socket onto a path here - so the answer has the same shape either way, and the
/// mirror, the subscription and the encoder below never learn which they got.
/// Tells the panes of a daemon on this machine where this window listens.
///
/// Separate from the two builders because it is orthogonal to both: whether a pane has a config
/// file to restore is about the daemon Muster started, and whether it can reach this window is
/// about which machine it is on.
fn reachable(panes: PaneEnvironment) -> PaneEnvironment {
    match command::listening_at() {
        Some(socket) => panes.reachable_at(&socket),
        None => panes,
    }
}

fn reach(daemon: &DaemonId, endpoint: &Endpoint) -> Result<Reached, String> {
    match endpoint {
        // A socket somebody named is a daemon somebody chose, of a version nobody promised.
        // Taken as asked for, and left alone: this is the deliberate way out of the
        // arrangement below, and second-guessing it would leave no way out at all.
        Endpoint::Local { socket_path: Some(path) } => Ok(Reached {
            socket_path: path.clone(),
            tunnel: None,
            // Nothing to restore, because Muster redirected nothing here - but a pane on this
            // machine is still a pane that should be able to drive the window it is in.
            panes: reachable(PaneEnvironment::none()),
            owns_config: false,
            remote_config: None,
            // Somebody named this socket, so whatever is behind it was somebody else's to
            // start and stays somebody else's to end.
            started: false,
        }),
        Endpoint::Local { socket_path: None } => {
            let environment = daemon::environment();
            let path = own_socket_path(&environment).ok_or_else(|| {
                "Muster cannot work out where its own daemon's socket would go, because \
                 nothing in the environment says where home is - neither HOME nor \
                 XDG_CONFIG_HOME. This window will render nothing. Give the daemon a `socket` \
                 in the config file to say outright."
                    .to_string()
            })?;
            // Before the spawn rather than beside it: `herdr server` takes no arguments and
            // reads its config once at startup, so a file written afterwards is a file that
            // daemon never sees.
            let config = write_daemon_configuration();
            let adopted = daemon::ensure_running(
                &path,
                daemon_binary().as_deref(),
                &environment,
                platform_locale().as_deref(),
                config.as_ref().map(|(path, _)| path.as_str()),
                commands_path().as_deref(),
            )?;
            // A daemon left running by an earlier Muster is holding somebody's agents, so it
            // is reused rather than restarted - but it read its config when it started. Asking
            // it to read again is what makes a setting saved between launches take effect
            // without costing anyone a pane.
            if adopted == daemon::Reached::Adopted
                && config.as_ref().is_some_and(|(_, moved)| *moved)
            {
                daemon::reload_configuration(&path);
            }
            let panes = reachable(match &config {
                Some(_) => PaneEnvironment::restoring(&environment),
                // Nowhere to write one, so nothing was redirected and nothing needs restoring.
                None => PaneEnvironment::none(),
            });
            Ok(Reached {
                socket_path: path,
                tunnel: None,
                panes,
                owns_config: config.is_some(),
                remote_config: None,
                started: adopted == daemon::Reached::Started,
            })
        }
        // A socket somebody named is somebody's own daemon on either machine, and gets the same
        // answer on both: forwarded as asked for, and left alone. Nothing is read off the far
        // end at all, because the one question that needed asking has been answered outright.
        Endpoint::Ssh { host, options, socket_path: Some(path) } => {
            let tunnel = open_tunnel(daemon, host, options, path.clone())?;
            Ok(Reached {
                socket_path: tunnel.local_socket_path().to_string(),
                tunnel: Some(tunnel),
                panes: PaneEnvironment::none(),
                owns_config: false,
                remote_config: None,
                started: false,
            })
        }
        // The arrangement the local arm has, one machine further away. What used to stop it was
        // packaging rather than principle - four platforms are pinned and a build carries one -
        // and that is answered by fetching the right asset here and pushing it over the master,
        // rather than by attaching whatever herdr somebody happened to install over there.
        Endpoint::Ssh { host, options, socket_path: None } => {
            // Asked for rather than assumed, and asked for using the rules Muster already
            // has: a shell one-liner spelling out where herdr keeps its socket would be a
            // second copy of the thing most likely to drift.
            let environment = remote_environment(host, options)?;
            let remote_socket = own_socket_path(&environment).ok_or_else(|| {
                format!(
                    "{host} answered, and nothing in its environment says where a herdr socket \
                     would go - it has no HOME. That machine's panes are absent from the window \
                     and nothing else is affected. Name the daemon's socket in the config \
                     file's `socket` key to say outright."
                )
            })?;
            // Opened before the daemon exists, which is what lets everything after this ask
            // "does it answer" through the forwarded path rather than inventing a second way to
            // probe. Measured against the devenv: ssh binds the local end when it connects and
            // reaches the far one per connection, so a remote socket that is not there yet
            // costs nothing until something dials it.
            let tunnel = open_tunnel(daemon, host, options, remote_socket)?;
            let adopted = remote::ensure_running(
                &tunnel.remote(),
                &environment,
                tunnel.local_socket_path(),
                cache_path().as_deref(),
                &daemon_configuration_text(),
            )?;
            // A daemon left running by an earlier Muster read its config when it started, and
            // the settings may have moved since. The same reasoning the local arm gives, with
            // one difference: there is no record here of what that daemon was started with, so
            // it is asked once rather than only when the file is known to have moved.
            if adopted == daemon::Reached::Adopted {
                daemon::reload_configuration(tunnel.local_socket_path());
            }
            Ok(Reached {
                socket_path: tunnel.local_socket_path().to_string(),
                tunnel: Some(tunnel),
                // Muster redirected this daemon's config, so a pane on it is handed the far
                // machine's own herdr config back - without which `herdr` typed in a devenv
                // pane would read Muster's derived file instead of the user's.
                panes: PaneEnvironment::restoring(&environment),
                owns_config: true,
                remote_config: remote::configuration_path(&environment),
                started: adopted == daemon::Reached::Started,
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
            health(daemon, "stale", detail);
            raise_problem(&reconnect::key(daemon.as_str()), Severity::Warning, detail);
        }
        TunnelState::Reachable => clear_problem(&reconnect::key(daemon.as_str())),
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
    pub(crate) control_socket_path: String,
    /// What this pane's own daemon calls it, for the bridge that streams its frames.
    ///
    /// The one place above the adapter that speaks the backend's vocabulary, and it does so
    /// because the bridge runs the daemon's own CLI rather than going through Muster. Carried
    /// rather than looked up when the answer is built, so that a pane whose daemon drops it
    /// between attaching and answering cannot be handed an id from a registry that has already
    /// forgotten it.
    pub(crate) backend_pane_id: String,
    /// Held because dropping it unlinks the socket and stops the listener.
    _control: Arc<PaneControlChannel>,
}

/// One daemon this process is following.
#[derive(Debug)]
struct Backend {
    mirror: Arc<Mutex<Mirror>>,
    /// The ssh master this daemon is reached through, for a remote one.
    ///
    /// Held because dropping it takes the connection down, and named because a pane's bridge
    /// needs the same master to run its frame stream through. Absent for a daemon on this
    /// machine, which is the difference the rest of this file never has to notice.
    tunnel: Option<Tunnel>,
    /// Whether Muster started this daemon rather than attaching to one already answering.
    ///
    /// Kept because ending a daemon is a decision somebody has to make about a process holding
    /// work, and "did we start it" is the one thing about it nothing else can reconstruct
    /// afterwards - `ps` cannot tell the daemon Muster launched a minute ago from the one it
    /// adopted that has been holding somebody's agent since yesterday.
    started: bool,
    /// Where this daemon was actually found, as opposed to how it was asked for.
    ///
    /// The resolution rather than the wish, which is why it lives here and not in the
    /// composition record beside it: a path discovered from this run's environment, or
    /// forwarded from another machine, describes nothing a later run could use.
    socket_path: String,
    /// How this daemon is asked for changes. One per daemon rather than one per pane,
    /// because what these ask for is structure and structure belongs to the daemon.
    channel: Arc<dyn BackendChannel>,
    /// Whether Muster wrote the config file this daemon reads.
    ///
    /// True only for a daemon Muster started itself. A daemon named by `socket` is somebody
    /// else's and a remote one was reached rather than started, so neither was redirected -
    /// and asking one of those to re-read its own config would be reaching into a session
    /// Muster does not own, possibly while its owner is editing that file.
    owns_config: bool,
    /// Where that config file is, when it is on another machine.
    ///
    /// None for a daemon on this one, whose file is written straight to disk. A remote
    /// daemon's has to be sent again before it is asked to re-read, and this is the only
    /// record of where it went.
    remote_config: Option<String>,
    /// This daemon's half of the pane-name registry.
    ///
    /// A view onto the one registry the session holds, scoped to this daemon, and the same one
    /// the subscription and the channel above were handed. Kept here so that everything with a
    /// backend in front of it can translate a name without reaching for the session's field
    /// and the daemon id separately.
    names: Names,
    /// Held because dropping it ends the subscription and every thread under it - the
    /// structure stream and one agent watcher per pane.
    _subscription: Subscription,
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
    /// Tabs Muster has asked a daemon to make, until the daemon says they exist.
    ///
    /// Shown when the mirror knows them rather than on trust, because a region whose tab the
    /// mirror has never heard of is dropped by the very next reconcile - so showing one
    /// immediately is a race against the event that would have made it true. Muster does not
    /// read a daemon's own focus to decide what a region shows (`architecture.md`, cursors
    /// are written, not read), so what it asked for is the only record there is.
    ///
    /// One per daemon, replaced rather than queued: two new tabs in flight at once is
    /// somebody pressing the key twice, and the second is the one they are looking for.
    wanted_tabs: BTreeMap<DaemonId, TabId>,

    /// A pane Muster made and wants the keyboard on, until the daemon has described it.
    ///
    /// The same shape as `wanted_tabs` and for the same reason: a split answers with the pane
    /// it made long before the event describing it arrives, and `Composition::reconcile`
    /// resolves a region against the mirror's pane list - so a keyboard pointed at a pane the
    /// mirror has not heard of falls to whichever pane the tab already had, and nothing
    /// afterwards points it back. What that looks like is a split whose new pane appears
    /// unfocused while the keyboard sits in the pane you split.
    ///
    /// One per daemon, replaced rather than queued, because two splits in flight at once is
    /// somebody pressing the key twice and the second is the one they are looking at.
    wanted_panes: BTreeMap<DaemonId, (RegionId, PaneId)>,

    /// Machines Muster has asked for a workspace and has not heard back about.
    ///
    /// Beside `wanted_tabs` and `wanted_panes` above, and the one of the three that guards the
    /// *asking* rather than what to do with the answer. A machine holding nothing is asked for a
    /// workspace, and until that workspace exists the machine still holds nothing - so an
    /// unguarded rule would ask again on every event any *other* machine sent, and a laptop that
    /// chatters would give a devenv a dozen workspaces before the first reply landed.
    ///
    /// Shared with the launch-time rule rather than kept beside it, which is the whole reason
    /// it is here and not a local. Launch asks before any daemon has spoken; this asks once one
    /// has spoken and said it holds nothing. Two sets would let a fresh window ask twice and
    /// open with a workspace it never wanted.
    ///
    /// Emptied per machine once that machine holds a tab, and never otherwise: a daemon that
    /// refuses is left in here deliberately, because a rule that retried a refusal would retry
    /// it on every event forever.
    ///
    /// Not emptied for a machine a window somebody asked for has claimed and not yet been given
    /// a tab on. That window is waiting for a tab of its own on a machine that already holds
    /// somebody else's, so "this machine holds a tab" is true throughout and would let the rule
    /// ask again on every event until the machine had a workspace per event.
    workspaces_asked_of: BTreeSet<DaemonId>,

    /// Whether somebody asked for this window, rather than it being the one Muster comes back
    /// to.
    ///
    /// One rule turns on it, and it is about where the window starts rather than about how it
    /// behaves: a window somebody asked for takes a tab of its own on every machine instead of
    /// the tab that machine last had focused. That tab is very often the one another window is
    /// showing, and herdr allows one client per terminal - so the alternative is a window of
    /// surfaces that paint nothing (kan `a_2IZ5TL6DQ`).
    ///
    /// Said by the shell rather than worked out here, because the two launches differ in
    /// nothing this layer can see. It stops mattering once every machine has been claimed
    /// below, which is what makes this a rule about where a window starts rather than about
    /// how it behaves.
    fresh: bool,

    /// What each machine was already holding when a window somebody asked for claimed a
    /// workspace on it.
    ///
    /// A record rather than a guard, and it is the difference between the two that matters:
    /// this window may open onto any tab that is not in here, and every tab that is belongs to
    /// whatever was using that machine before. So the entry both suppresses the wrong answer
    /// and identifies the right one, without needing the claim's reply to arrive while
    /// something is still waiting for it.
    ///
    /// One entry per machine for the life of the window, never removed. After the first fill
    /// all it says is which tabs this window inherited, and preferring the others is what
    /// somebody opening a window by hand meant anyway.
    claimed: BTreeMap<DaemonId, BTreeSet<TabId>>,

    /// Which agents have been seen, and so which are `done`.
    ///
    /// Beside the mirrors rather than inside one, because it spans them: a window is focused
    /// or it is not, and that answers for a laptop's panes and a devenv's at once.
    attention: Attention,

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

    /// The search somebody has open, if anybody has.
    ///
    /// One at a time, because the find bar is one bar over the pane with the keyboard. Held
    /// here rather than in `Presentation` because it is not worth a restart remembering -
    /// `presentation.rs` says as much about panels that open on a chord - and not in the
    /// shell because which hit is selected decides which scroll goes out, and deciding is
    /// this side's job.
    search: Option<Search>,

    /// What Muster calls each pane, across every daemon at once.
    ///
    /// One registry rather than one per daemon, because a name has to be unique over all of
    /// them: that is what lets a caller name a pane on the devenv without saying which machine
    /// holds it. Each `Backend` above holds a view onto this scoped to its own daemon.
    ///
    /// Shared behind a lock because two threads mint into it - a daemon's subscription thread
    /// when a pane appears, and whichever thread dispatched a split.
    names: Arc<Mutex<PaneNames>>,

    /// The same for tabs, on the same terms, and a second lock rather than one over both so
    /// that decoding a pane event never waits on a tab being named.
    tab_names: Arc<Mutex<TabNames>>,

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
    armed: Option<TabKey>,
}

/// One pane's live search.
///
/// The hits are kept rather than looked for again on every step. A pane goes on printing
/// while somebody reads it, so a needle asked for twice can answer differently - and a
/// counter that says "3 of 47" while walking a list of 51 is worse than one that is a moment
/// behind. Re-typing is what re-reads.
#[derive(Debug)]
struct Search {
    daemon: DaemonId,
    pane: PaneId,
    found: Found,
    /// Which hit is selected, or none when nothing matched.
    selected: Option<usize>,
}

pub(crate) static SESSION: LazyLock<Mutex<Session>> =
    LazyLock::new(|| Mutex::new(Session::default()));

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
/// Dropping the session is the whole teardown: a `Backend` owns its subscription, its ssh
/// master and its panes' sockets, and dropping one ends the threads behind all three. So this
/// is the shutdown path that already existed, called deliberately rather than at exit.
pub(crate) fn reset() {
    *poison::lock(&SESSION, "session") = Session::default();

    *poison::lock(&DAEMON_BINARY, "daemon-binary") = None;
    *poison::lock(&PLATFORM_LOCALE, "locale") = None;
    *poison::lock(&COMMANDS, "commands") = None;
    *poison::lock(&STATE, "saved-arrangement") = None;
    *poison::lock(&NAMES_FILE, "saved-names") = None;
    *poison::lock(&SHARED_NAMES, "shared-names") = None;
    *poison::lock(&BINDINGS, "bindings") = None;
    *poison::lock(&PANE_INPUT, "input-settings") = None;
    *poison::lock(&PANES, "pane-settings") = None;
    *poison::lock(&DAEMON_CONFIG, "daemon-config") = None;
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
    command::listen("");
    ffi::muster_set_event_callback(None);
}

impl Session {
    /// Starts following a daemon, or leaves the one already being followed alone.
    ///
    /// Seeded with a snapshot the caller already has, before the subscription starts. The
    /// subscription takes its own moments later and replaces this one; doing it in the other
    /// order would let the older answer land last and stick, since a mirror with nothing
    /// happening in it is never corrected.
    ///
    /// The endpoint and the socket path are both passed because they are different things.
    /// The endpoint is what someone asked for and is what composition writes down; the path
    /// is where this run found it, and is worth nothing to a later one.
    ///
    /// Returns what the seed established, for the caller to announce once it has let go of
    /// the session. Nothing else will: the subscription's own bootstrap diffs against this
    /// mirror rather than an empty one, so every pane the seed already placed is a pane it
    /// sees no change in. Dropping these on the floor is how a window opened onto running
    /// agents came up believing every one of them was unknown.
    fn follow(&mut self, daemon: &Daemon, reached: Reached, seed: Snapshot) -> Vec<Change> {
        let id = daemon.id.clone();
        self.composition.attach_daemon(daemon.clone());
        if self.backends.contains_key(&id) {
            return Vec::new();
        }

        let mut mirror = Mirror::new();
        let seeded = mirror.bootstrap(seed);
        let mirror = Arc::new(Mutex::new(mirror));

        // Sharing the record where there is one, so that naming a pane this daemon has just
        // described cannot race another Muster naming the same one.
        let names = match shared_names() {
            Some(shared) => Names::sharing(
                id.clone(),
                Arc::clone(&self.names),
                Arc::clone(&self.tab_names),
                shared,
            ),
            None => Names::new(id.clone(), Arc::clone(&self.names), Arc::clone(&self.tab_names)),
        };
        let reporting = id.clone();
        let subscription = Subscription::start(
            &reached.socket_path,
            Arc::clone(&mirror),
            Arc::new(move |notice| announce(&reporting, notice)),
            names.clone(),
        );
        self.backends.insert(
            id,
            Backend {
                mirror,
                tunnel: reached.tunnel,
                channel: Arc::new(HerdrBackend::new(
                    HerdrClient::new(reached.socket_path.clone()),
                    reached.panes,
                    names.clone(),
                )),
                owns_config: reached.owns_config,
                remote_config: reached.remote_config,
                started: reached.started,
                socket_path: reached.socket_path,
                names,
                _subscription: subscription,
            },
        );
        seeded
    }

    /// Drops what a daemon no longer holds: the names of panes and tabs, and the text sizes
    /// somebody set on panes.
    ///
    /// Only where the daemon is answering. A mirror that has gone stale is not evidence a pane
    /// is gone - it is a connection nobody is hearing from - and forgetting a name on a dropped
    /// VPN would strand an agent that is still working on the far side with a name nothing can
    /// resolve. A forgotten tab name is milder and wrong in the same direction: the saved
    /// arrangement would stop finding the tab a region was showing. A forgotten text size is
    /// milder still and wrong the same way: a pane comes back at the configured size after a
    /// blip, having been made bigger on purpose.
    fn forget_what_closed(&mut self) {
        for backend in self.backends.values() {
            let mirror = poison::lock(&backend.mirror, "mirror");
            if mirror.health() != Health::Connected {
                continue;
            }
            let panes: Vec<PaneId> = mirror.panes().map(|pane| pane.id.clone()).collect();
            let tabs: Vec<TabId> = mirror.tabs().map(|tab| tab.id.clone()).collect();
            drop(mirror);
            // The mirror holds Muster's names, so this asks the registry for each one's backend
            // id rather than the other way round.
            let held = panes.iter().filter_map(|pane| backend.names.backend_pane(pane).ok());
            backend.names.prune_panes(held);
            let held = tabs.iter().filter_map(|tab| backend.names.backend_tab(tab).ok());
            backend.names.prune_tabs(held);
        }

        // Text sizes follow the names, rather than answering the question a second time. The
        // registry already knows the difference between a pane that has gone and one the daemon
        // has not described yet - a split's pane is named before the daemon announces it, and
        // this runs on the publish in between - and getting that difference wrong here means a
        // pane opening at the size of the pane it was split from and losing it a frame later.
        let gone: Vec<PaneKey> = self
            .font_sizes
            .entries()
            .filter(|(pane, _)| {
                // A daemon nothing is following is not evidence either. An entry for one this
                // window is about to attach again is what makes a size survive a relaunch.
                self.backends
                    .get(&pane.daemon)
                    .is_some_and(|backend| backend.names.backend_pane(&pane.pane).is_err())
            })
            .map(|(pane, _)| pane.clone())
            .collect();
        self.font_sizes.retain(|pane| !gone.contains(pane));
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

    /// Brings composition, and what this process holds open, in line with one daemon.
    fn reconcile(&mut self, daemon: &DaemonId) {
        self.prune(daemon);
        self.open_channels(daemon);
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

            self.composition
                .regions()
                .filter(|region| &region.daemon == daemon)
                .filter_map(|region| mirror.layout(&region.tab))
                .flat_map(|layout| layout.root.panes())
                .filter(|pane| !attached.contains_key(pane) && mirror.pane(pane).is_some())
                .cloned()
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
                        "detail" => refusal,
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
        let socket_path = backend.socket_path.clone();
        // Resolved once, here, because both things this opens speak to the daemon directly:
        // the second input channel below, and the bridge the shell starts from the answer.
        let backend_pane = backend.names.backend_pane(pane).map_err(|_| {
            format!(
                "{daemon} does not hold a pane called {pane}, so there is nothing to open a \
                 channel to. A name the window is still showing and the registry has already \
                 forgotten means a pane closed between the two - the next publish drops it."
            )
        })?;
        let path = self.next_socket_path();
        let dialed = PaneKey::new(daemon, pane);
        let stopped = dialed.clone();
        let control = PaneControlChannel::bind(
            path.clone(),
            Reports {
                connected: Box::new(move || typeable(&dialed.daemon, &dialed.pane)),
                exited: Box::new(move |ended| bridge_ended(&stopped, &ended)),
            },
        )
        .map_err(|error| {
            format!(
                "could not bind the socket this pane's bridge dials back on ({error}). \
                     Usual causes: a full or read-only temporary directory."
            )
        })?;
        let control = Arc::new(control);

        // The second channel, for the keys and text whose correct encoding depends on modes
        // the control stream cannot show us. Pointed at the daemon this pane belongs to
        // rather than at whatever the environment names: a remote pane asked of the local
        // daemon is a pane whose arrows quietly go to the wrong machine, and the failure
        // reads as a guessed encoding rather than as an error.
        let server = HerdrPaneChannel::new(HerdrClient::new(socket_path), backend_pane.as_str());

        // The pane's modes are not readable, so this is the documented guess, with the one
        // field in it that is a preference taken from the config file. One day the rest is
        // fed from the daemon; nothing above here changes when it is.
        let settings = pane_input();
        let encoder = KeyEncoder::new(settings.profile()).map_err(|error| {
            format!(
                "could not build a key encoder ({error}), so nothing typed into this pane \
                 would reach it. libghostty-vt is behind this; check that ./dev built it."
            )
        })?;

        self.panes.entry(daemon.clone()).or_default().insert(
            pane.clone(),
            Arc::new(AttachedPane {
                input: PaneInput::new(
                    Arc::clone(&control) as Arc<_>,
                    Some(Arc::new(server) as Arc<_>),
                    Arc::new(encoder),
                    &settings,
                ),
                control_socket_path: path,
                backend_pane_id: backend_pane.as_str().to_string(),
                _control: control,
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
    /// Scoped to the daemon rather than searched, for the reason every other lookup here is:
    /// two daemons hand out `w1:p1`, and a search would let one answer for the other's pane.
    fn agent_state(&self, pane: &PaneKey) -> Option<AgentState> {
        let mirror = poison::lock(&self.backends.get(&pane.daemon)?.mirror, "mirror");
        mirror.agent_state(&pane.pane)
    }

    /// Shows a tab Muster asked for, once the daemon has described it.
    ///
    /// Returns whether anything moved, so the caller knows whether to republish.
    /// Puts the keyboard on a pane Muster made, once the daemon has described it.
    ///
    /// Returns whether anything moved, so the caller knows whether to republish. Held rather
    /// than applied and hoped for: `Composition::reconcile` runs before every publish and
    /// resolves each region against the mirror's pane list, so pointing at a pane the mirror
    /// does not hold yet is undone by the next publish rather than remembered.
    fn keyboard_to_wanted_pane(&mut self, daemon: &DaemonId) -> bool {
        let Some((region, pane)) = self.wanted_panes.get(daemon).cloned() else { return false };
        {
            let Some(backend) = self.backends.get(daemon) else { return false };
            let mirror = poison::lock(&backend.mirror, "mirror");
            // Not yet described. Left in place rather than dropped, on the same terms as a
            // wanted tab: the event is on its way, and forgetting it here is a split whose
            // pane never takes the keyboard.
            if mirror.pane(&pane).is_none() {
                return false;
            }
        }
        self.wanted_panes.remove(daemon);
        self.composition.focus_pane(region, pane);
        true
    }

    fn show_wanted_tab(&mut self, daemon: &DaemonId) -> bool {
        let Some(tab) = self.wanted_tabs.get(daemon).cloned() else { return false };
        {
            let Some(backend) = self.backends.get(daemon) else { return false };
            let mirror = poison::lock(&backend.mirror, "mirror");
            // Not yet described. Left in place rather than dropped: the event is on its way,
            // and forgetting it here is a new tab nothing ever shows.
            if mirror.tab(&tab).is_none() {
                return false;
            }
        }
        self.wanted_tabs.remove(daemon);
        self.composition.surface(daemon, tab).is_some()
    }

    /// Puts a region onto the tab holding this pane, so that something can show it.
    ///
    /// The mirror is what knows which tab a pane is in, so the lookup is here and the policy
    /// - which region, or a new one - is in the composition record where the rest of it is.
    ///
    /// Refuses by name rather than silently doing nothing. A pane the daemon has never heard
    /// of and a pane that closed while a click was in flight look identical from a sidebar
    /// row, and both leave the keyboard where it was.
    fn surface(&mut self, daemon: &DaemonId, pane: &PaneId) -> Result<RegionId, String> {
        let tab = {
            let backend = self.backends.get(daemon).ok_or_else(|| {
                format!(
                    "this window is not following a daemon called {daemon}, so there is \
                     nothing to show {pane} in and the keyboard stayed where it was."
                )
            })?;
            let mirror = poison::lock(&backend.mirror, "mirror");
            let held = mirror.pane(pane).ok_or_else(|| {
                format!(
                    "{daemon} holds no pane called {pane}, so the keyboard stayed where it \
                     was. Most likely it closed while this was in flight, which an entry in a \
                     list outlives by a moment."
                )
            })?;
            held.tab.clone()
        };
        self.composition.surface(daemon, tab).ok_or_else(|| {
            format!(
                "{daemon} is followed but not attached to this window's composition, so no \
                 region could be opened onto {pane}."
            )
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
            |daemon| {
                let tunnel = self.backends.get(daemon)?.tunnel.as_ref()?;
                Some(Transport {
                    host: tunnel.host().to_string(),
                    control_path: tunnel.control_path().to_string(),
                })
            },
            |daemon| {
                let backend = self.backends.get(daemon)?;
                // Whichever path the bridge's own CLI will be able to open, which is not the
                // same path in both cases: a bridge for a local pane runs here and takes the
                // socket as it is, and one for a remote pane runs its CLI on the far end,
                // where the near end of a tunnel names nothing at all. Handed over rather
                // than found either way, because Muster's daemon listens on a session of its
                // own and a CLI left to look for itself reaches a different one.
                match &backend.tunnel {
                    Some(tunnel) => Some(tunnel.remote_socket_path().to_string()),
                    None => Some(backend.socket_path.clone()),
                }
            },
            |daemon, pane| {
                let key = PaneKey::new(daemon, pane);
                ViewPane {
                    id: pane.clone(),
                    control_socket_path: self
                        .channel(daemon, pane)
                        .map(|held| held.control_socket_path.clone()),
                    backend_pane_id: self
                        .backends
                        .get(daemon)
                        .and_then(|backend| backend.names.backend_pane(pane).ok())
                        .map(|named| named.as_str().to_string()),
                    font_size_offset: self.font_sizes.offset(&key),
                    bridge_restarts: self.respawns.count(&key),
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

    /// One named pane, whether or not the keyboard is in it.
    ///
    /// Behind an `Arc` for the same reason [`Self::keyboard_pane`] is: the caller lets go of
    /// this lock before it sends anything.
    fn attached_pane(&self, daemon: &DaemonId, pane: &PaneId) -> Option<Arc<AttachedPane>> {
        self.panes.get(daemon)?.get(pane).map(Arc::clone)
    }

    /// Which of one daemon's regions shows this pane.
    ///
    /// Scoped to a daemon rather than searched across all of them, because two daemons hand
    /// out the same pane ids - `w1:p1` means something on each - and a search would let
    /// whichever happened to be first answer for the other's pane. A pane in none of that
    /// daemon's regions is one this window is not showing, and nothing here will act on it.
    fn region_holding(&self, daemon: &DaemonId, pane: &PaneId) -> Option<RegionId> {
        let backend = self.backends.get(daemon)?;
        let held = poison::lock(&backend.mirror, "mirror");
        self.composition
            .regions()
            .find(|region| {
                &region.daemon == daemon
                    && held.pane(pane).is_some_and(|held| held.tab == region.tab)
            })
            .map(|region| region.id)
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

/// One named pane, if this window has a channel open to it.
///
/// For the input that is addressed rather than focused, which today is the wheel. Absent means
/// no channel, not no pane: a pane whose bridge has not finished starting is the ordinary case.
pub(crate) fn attached_pane(daemon: &DaemonId, pane: &PaneId) -> Option<Arc<AttachedPane>> {
    poison::lock(&SESSION, "session").attached_pane(daemon, pane)
}

/// Asks the daemon showing this window for a change.
///
/// Nothing is applied here, and nothing is answered with new state: what happened arrives
/// afterwards on the daemon's own events, which is what keeps one description of the session
/// rather than two that can disagree (`architecture.md`, view = f(daemon state)).
///
/// The one exception is where the keyboard ends up, which is Muster's own state and not the
/// daemon's. A split you asked for takes it, because that is what pressing the key meant.
///
/// The channel is taken out from under the lock before the request goes, because a request
/// is a round trip and holding the session across one would stall every event arriving from
/// every other daemon behind a wedged one.
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

pub(crate) fn submit(
    daemon: &DaemonId,
    intent: &BackendIntent,
    keyboard: Keyboard,
) -> Result<Option<PaneId>, String> {
    let (region, source, channel) = {
        let session = poison::lock(&SESSION, "session");
        // Which pane this request came from, for a pane it may be about to make. A split names
        // the pane it splits; ⌘T and a new workspace name nothing, so the answer is the pane in
        // front of whoever asked. Read here rather than after the round trip, because by then
        // the keyboard may have moved.
        let source = match intent {
            BackendIntent::SplitPane { pane, .. } => Some(PaneKey::new(daemon, pane)),
            BackendIntent::CreateTab { .. } | BackendIntent::CreateWorkspace { .. } => session
                .composition
                .focused_region()
                .and_then(|region| Some(PaneKey::new(&region.daemon, region.pane.as_ref()?))),
            _ => None,
        };
        // Which region this is about, for the keyboard afterwards. None is an answer rather
        // than a failure for an intent that names nothing existing - there is no region to
        // find for a workspace that does not exist yet, and the one it produces is opened by
        // the reconcile behind the daemon's own event.
        let region = match intent {
            // A rename is about a thing rather than about what is on screen, and requiring a
            // region would refuse the case the feature exists for: the sidebar lists every
            // pane every daemon holds, and the ones worth naming are the ones no region is
            // showing.
            // Arranging the list is about things rather than about what is on screen, for the
            // same reason a rename is: the rows worth dragging are very often the ones no
            // region is showing, and requiring one would refuse the case the gesture is for.
            // Typing into a pane is about the pane, not about what is on screen, and this is the
            // sharpest case of that: what it is *for* is reaching a pane no region shows. An
            // agent told to instruct two others has to be able to reach them wherever they are.
            BackendIntent::CreateWorkspace { .. }
            | BackendIntent::CreateTab { .. }
            | BackendIntent::RenamePane { .. }
            | BackendIntent::RenameTab { .. }
            | BackendIntent::SwapPanes { .. }
            | BackendIntent::SendText { .. }
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
            | BackendIntent::ZoomPane { pane }
            | BackendIntent::FocusPane { pane } => {
                Some(session.region_holding(daemon, pane).ok_or_else(|| not_showing(daemon))?)
            }
            // Closing a tab keeps the region requirement for the reason closing a pane does:
            // it destroys every pane in the tab, and destroying what nobody is looking at is a
            // different risk from arranging it. Dragging a divider needs the region for a
            // plainer reason - there is no divider to move in a tab nothing is drawing.
            BackendIntent::CloseTab { tab } | BackendIntent::SetSplitRatio { tab, .. } => Some(
                session
                    .composition
                    .region_showing(daemon, tab)
                    .ok_or_else(|| not_showing(daemon))?,
            ),
        };
        let channel = session.channel_of(daemon).ok_or_else(|| {
            format!(
                "the daemon {daemon} is in this window's composition and is not being \
                     followed, which is a bug in the core rather than a state to recover from"
            )
        })?;
        (region, source, channel)
    };

    let outcome = channel.submit(intent);
    log::info(
        "intent.submitted",
        fields! {
            "intent" => intent.redacted(),
            "backend" => channel.description(),
            "created" => outcome.as_ref().ok().and_then(|outcome| outcome.created.clone())
                .map(|pane| pane.to_string()).unwrap_or_default(),
            "refused" => outcome.as_ref().err().map(ToString::to_string).unwrap_or_default(),
        },
    );

    // A daemon that says it does not hold what was named is describing this window rather
    // than the request: whatever is on screen for that thing is stale, and every later
    // request about it is refused the same way. It has to be asked what it does hold, because
    // it will not volunteer it - herdr drops a pane whose terminal is gone without an event
    // for it, which leaves a pane on screen that cannot be focused, closed or typed into.
    if let Err(Refusal::NotThere(detail)) = &outcome {
        resnapshot(daemon, detail);
    }

    // The new pane is not in the mirror yet - its event is still in flight - so the region is
    // the one that was split rather than one looked up. Taking the pane on trust is what
    // `Composition::focus_pane` is for, and the reconcile behind the event that follows is
    // where daemon truth gets applied to it.
    // A tab this request made is remembered rather than shown: the mirror has not heard of
    // it yet, and a region pointed at a tab the mirror does not know is dropped by the next
    // reconcile. The reconcile behind the daemon's own event is where it becomes visible.
    //
    // Not for a move, which is the one request here that makes a tab without being about one:
    // it makes a place to put a pane. Bringing that tab on screen would put the tab somebody
    // was working in behind it, so "pull that pane out of the split" would answer by moving
    // them somewhere they did not ask to go - and an agent pulling another agent's pane out
    // would lose its own place doing it. The tab is listed and named, and `muster tab focus`
    // is how anybody who does want to go there says so.
    if !matches!(intent, BackendIntent::MovePane { .. })
        && let Some(tab) = outcome.as_ref().ok().and_then(|outcome| outcome.created_tab.clone())
    {
        let mut session = poison::lock(&SESSION, "session");
        session.wanted_tabs.insert(daemon.clone(), tab);
    }

    // What the daemon said about the arrangement it just made, taken now rather than waited
    // for. herdr answers a swap or a resize with the settled tree and broadcasts the same tree
    // about a hundred milliseconds later, so a window that only listens renders the
    // arrangement being moved away from for six frames and then jumps
    // (`observations/herdr-0.8.0.md` section 14). Still daemon truth - the mirror is what
    // applies it, and it arms itself against the broadcast that is now behind it.
    let mut moved = false;
    if let Ok(outcome) = &outcome
        && let Some(settled) = outcome.settled.clone()
    {
        let session = poison::lock(&SESSION, "session");
        if let Some(backend) = session.backends.get(daemon) {
            moved = !poison::lock(&backend.mirror, "mirror").settle(settled).is_empty();
        }
    }

    // What the daemon said a pane is now called, taken from the answer for the same reason as
    // the arrangement above and with less choice about it: herdr emits no event for a pane
    // rename and has no topic for one, so this reply is the only thing that will ever say so
    // (`observations/herdr-0.8.0.md` section 16). Without this, naming a pane changes the
    // daemon and leaves the window reading what it read before.
    if let Ok(outcome) = &outcome
        && let Some((pane, name)) = outcome.renamed.clone()
    {
        let session = poison::lock(&SESSION, "session");
        if let Some(backend) = session.backends.get(daemon) {
            moved |= !poison::lock(&backend.mirror, "mirror").rename(&pane, name).is_empty();
        }
    }

    // The pane a split made, remembered rather than pointed at. It is not in the mirror yet -
    // its event is still in flight - and every publish resolves a region against the mirror's
    // pane list, so pointing at it now is undone before anything renders. `publish` puts the
    // keyboard there on the first pass after the daemon has described it.
    let created = outcome.as_ref().ok().and_then(|outcome| outcome.created.clone());

    // A pane opens at the size of the pane it came from, which is what Ghostty does
    // (`window-inherit-font-size`, on by default) and what somebody who has finally made a
    // pane readable means by splitting it. Whatever the keyboard does: an agent's `muster pane
    // new` leaves the cursor alone and still makes a pane beside one that was sized.
    //
    // A pane another client made inherits nothing. There is no request to have come from, and
    // taking whatever this window happened to be focused on would be an answer nobody asked
    // for.
    if let (Some(source), Some(created)) = (&source, &created) {
        let mut session = poison::lock(&SESSION, "session");
        let made = PaneKey::new(daemon, created);
        session.font_sizes.inherit(&made, source);
    }

    if let (Some(region), Some(created), Keyboard::Follows) = (region, &created, keyboard) {
        let mut session = poison::lock(&SESSION, "session");
        session.wanted_panes.insert(daemon.clone(), (region, created.clone()));
        moved = true;
    }
    if moved {
        publish();
    }
    // The pane, so a caller can name it in its next breath. The only thing here a caller could
    // not have learned some other way: the arrangement reaches it as a view, and this reaches it
    // nowhere else - herdr's own id for the pane is not Muster's name for it, and the name was
    // minted inside this call.
    outcome.map(|_| created).map_err(|refusal| refusal.to_string())
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
/// The resnapshot below is what answers that, and the interval between exits is what answers
/// the harder question of whether replacing it is going to help (`muster_core::respawn`).
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
        // The wait starts again. A pane keeps its channel while its surface is thrown away and
        // built again, so the replacement bridge has to dial too - and a replacement that never
        // arrives is the same deaf pane, which is the case `control_socket.rs` names as the
        // reason its accept loop runs more than once.
        watchdog::opened(key.clone());
        resnapshot(&daemon, &format!("nothing is painting {pane} any more"));
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
    // refused attach can then name the client holding its terminal rather than pointing at a
    // log file.
    watchdog::ended(pane.clone(), ended.clone());
    // Before deciding anything, because the decision turns on whether the daemon still holds
    // this pane and the mirror is the only place that is written down.
    resnapshot(&pane.daemon, &format!("nothing is painting {} any more", pane.pane));
    replace_bridge(pane, ended.ending);
}

/// Starts another bridge for a pane whose last one ended, or says why it will not.
///
/// Nothing is spawned here. The shell owns the surfaces and a bridge is a surface's command,
/// so what this does is count the replacement and publish - and the view carrying a number the
/// shell has not seen for this pane is what makes it build one.
fn replace_bridge(pane: &PaneKey, ending: Ending) {
    let decision = {
        let mut session = poison::lock(&SESSION, "session");
        if !session.holds(pane) {
            // The pane closed, which is the other reason a bridge exits on its own. The region
            // showing it has already been reconciled away by the resnapshot; what is left is
            // the record of what was tried, which belongs to a pane that no longer exists.
            session.respawns.forget(pane);
            return;
        }
        session.respawns.ended(pane, clock::monotonic_now(), ending)
    };

    match decision {
        Decision::Start(count) => {
            log::info(
                "bridge.replacing",
                fields! { "pane" => pane.to_string(), "attempt" => count.to_string() },
            );
            publish();
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
        // got it, which is the arrangement herdr allows and the one Muster asked for on their
        // behalf. What would be wrong is taking it back, and this is that not happening.
        Decision::Yield => log::info(
            "bridge.yielded",
            fields! { "pane" => pane.to_string(), "detail" => respawn::yielded(pane) },
        ),
    }
}

/// Asks a daemon what it actually holds, and shows that instead.
///
/// The whole picture rather than the one thing that was refused, because the refusal only
/// proves the picture is wrong somewhere - a pane that went may have taken its tab with it,
/// and patching out the single entry that was named would leave the rest of the same staleness
/// on screen. `bootstrap` already diffs against what the mirror holds, so what this costs when
/// only one thing moved is one round trip and one change.
///
/// A daemon that will not answer is left alone. The window is already showing something wrong;
/// replacing it with nothing on the strength of a failed request would be worse, and the
/// subscription's own health reporting is what speaks for a daemon that has stopped answering.
fn resnapshot(daemon: &DaemonId, why: &str) {
    let Some((socket_path, mirror, names)) = ({
        let session = poison::lock(&SESSION, "session");
        session.backends.get(daemon).map(|backend| {
            (backend.socket_path.clone(), Arc::clone(&backend.mirror), backend.names.clone())
        })
    }) else {
        return;
    };

    let (snapshot, dropped) = match fetch_snapshot(&socket_path, &names) {
        Ok(answer) => answer,
        Err(failure) => {
            log::warn(
                "mirror.resnapshot.failed",
                fields! {
                    "daemon" => daemon.to_string(),
                    "detail" => failure.to_string(),
                    "impact" => "the window keeps showing what it was showing, including \
                                 whatever the daemon has just said it does not hold",
                    "check" => "whether this daemon is still answering at all - a health \
                                record for it follows if it is not",
                },
            );
            return;
        }
    };

    let changes = {
        let mut mirror = poison::lock(&mirror, "mirror");
        mirror.bootstrap(snapshot)
    };
    log::info(
        "mirror.resnapshot",
        fields! {
            "daemon" => daemon.to_string(),
            "changes" => changes.len().to_string(),
            "dropped" => dropped.to_string(),
            "why" => why.to_string(),
        },
    );

    reconcile(daemon);
    publish();
    for change in changes {
        report(daemon, &change);
    }
}

/// Why an intent about something on screen could not be sent.
fn not_showing(daemon: &DaemonId) -> String {
    format!(
        "the daemon {daemon} is not showing that pane or tab in this window, so nothing was \
         asked of anything. Either it closed while this was in flight, or the request names \
         something in a session this window is not attached to."
    )
}

/// Points this window's keyboard at a pane, and tells the daemon somebody looked.
///
/// The two halves are one action to whoever clicked, and they are kept separate underneath:
/// the keyboard moves whatever the daemon says, because it is Muster's own cursor, and the
/// daemon is told as a courtesy it may refuse. A refused write is worth a log line and not
/// worth undoing a focus move the user can see happened.
pub(crate) fn focus(daemon: &DaemonId, pane: &PaneId) -> Result<(), String> {
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
    publish();
    submit(daemon, &BackendIntent::FocusPane { pane: pane.clone() }, Keyboard::Follows).map(drop)
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
    publish();
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
    focus(&daemon, &pane)
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
        let from = session
            .composition
            .focused_region()
            .map(|region| TabKey::new(&region.daemon, &region.tab));
        session.roster(&session.view()).step(from.as_ref(), direction).map(landing)
    };
    let (daemon, pane) = stepped.ok_or_else(|| {
        "this window is attached to no tabs to step through, so the keyboard stayed where it \
         was. A window whose daemons have not described a session yet looks like this, and so \
         does one whose tabs all closed."
            .to_string()
    })??;
    focus(&daemon, &pane)
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
/// than the second - and it has to be, because a pane id is only unique within its daemon and
/// resolving one against the wrong mirror would find a different pane and move it.
pub(crate) fn arrange_pane(daemon: &DaemonId, pane: &PaneId, onto: &PaneId) -> Result<(), String> {
    let intent = {
        let session = poison::lock(&SESSION, "session");
        let backend = session.backends.get(daemon).ok_or_else(|| {
            format!(
                "this window is not following a daemon called {daemon}, so nothing was \
                 rearranged. Either it detached while this was in flight, or the request \
                 named a daemon this window does not have."
            )
        })?;
        let mirror = poison::lock(&backend.mirror, "mirror");
        let holding = |pane: &PaneId| {
            mirror.pane(pane).map(|held| held.tab.clone()).ok_or_else(|| {
                format!(
                    "{daemon} holds no pane called {pane}, so nothing was rearranged. Either \
                         it closed while this was in flight, or the two panes are on different \
                         machines - a pane is a PTY its daemon owns, so there is no move that \
                         would carry one to the other. `muster window` says which daemon holds \
                         each."
                )
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
pub(crate) fn close_tab(daemon: &DaemonId, tab: &TabId) -> Result<(), String> {
    submit(daemon, &BackendIntent::CloseTab { tab: tab.clone() }, Keyboard::StaysPut).map(drop)
}

/// Takes a pane out of whatever tab it is in and gives it one of its own.
///
/// Beside [`arrange_pane`] rather than inside it, because the two take different arguments and
/// mean different things to whoever asked: one names where the pane is going and one says it is
/// going nowhere in particular. What they share is the intent, and the adapter is where the two
/// destinations become one request.
///
/// No region and no keyboard move. The tab it makes comes on screen through the ordinary path -
/// the daemon says it exists, and `show_wanted_tab` puts a region on it - and the keyboard stays
/// where it is, because pulling a pane out of a split is arranging the window rather than going
/// somewhere.
pub(crate) fn move_pane_to_new_tab(
    daemon: &DaemonId,
    pane: &PaneId,
    name: Option<String>,
) -> Result<(), String> {
    let intent =
        BackendIntent::MovePane { pane: pane.clone(), to: MoveDestination::NewTab { name } };
    submit(daemon, &intent, Keyboard::StaysPut).map(drop)
}

/// Brings a named tab on screen, landing the keyboard on its first pane.
///
/// The mouse's half of what `next_tab` does with the keyboard, through the same [`landing`]
/// rule so that the two agree about where a tab is entered.
pub(crate) fn focus_tab(daemon: &DaemonId, tab: &TabId) -> Result<(), String> {
    let found = {
        let session = poison::lock(&SESSION, "session");
        let key = TabKey::new(daemon, tab);
        match session.roster(&session.view()).tabs().find(|held| held.key == key) {
            Some(held) => landing(held),
            None => Err(format!(
                "this window is not showing a tab called {key}, so the keyboard stayed where \
                 it was. Most likely it closed while the click was in flight."
            )),
        }
    };
    let (daemon, pane) = found?;
    focus(&daemon, &pane)
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
    focus(&daemon, &pane)
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
            let held = roster.tabs().find(|tab| &tab.key == key).map_or(0, |tab| tab.panes.len());
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
    let (roster, numbering) = {
        let session = poison::lock(&SESSION, "session");
        let roster = session.roster(&session.view());
        let numbering = session.numbering(&roster);
        (roster, numbering)
    };
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
    ffi::emit(&Event {
        payload: Some(event::Payload::RosterChanged(convert::roster(&roster, &numbering))),
    });
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
/// rows agree about which one comes first. Not the daemon's focused pane: daemon focus is a
/// single value shared with every client, so reading it back would let another client decide
/// where this window's keyboard goes (`architecture.md`, cursors are written, not read).
///
/// Names the pane rather than focusing it, because [`focus`] takes the session lock and every
/// caller here is holding it.
fn landing(tab: &RosterTab) -> Result<(DaemonId, PaneId), String> {
    let pane = tab.panes.first().ok_or_else(|| {
        format!(
            "{} holds no panes, so there is nothing for the keyboard to land on. Most likely \
             they closed while this was in flight.",
            tab.key
        )
    })?;
    Ok((tab.key.daemon.clone(), pane.key.pane.clone()))
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
/// What a request that names a machine and no pane means. The focused region when the
/// keyboard is already on that machine, and otherwise that machine's first region - so
/// `--daemon` reaches the pane somebody would be typing into if they went there.
///
/// Read from the composition and never from the daemon's own focus cursor, which is one value
/// shared with every other client: routing by it would let a herdr TUI in another window
/// decide where this one's requests land (`architecture.md`, cursors are written, not read).
pub(crate) fn focused_pane_on(daemon: &DaemonId) -> Option<PaneId> {
    let session = poison::lock(&SESSION, "session");
    session
        .composition
        .focused_region()
        .filter(|region| &region.daemon == daemon)
        .or_else(|| session.composition.regions().find(|region| &region.daemon == daemon))?
        .pane
        .clone()
}

/// Whether no region of this window is showing this machine.
///
/// The question that separates a machine there is nothing to act on from one whose tab this
/// window has not been told the panes of yet. Both leave a request that named no pane without
/// one, and only the first should be answered by making a workspace - a machine that has a
/// region is a machine whose event is on its way.
pub(crate) fn showing_nothing(daemon: &DaemonId) -> bool {
    let session = poison::lock(&SESSION, "session");
    !session.composition.regions().any(|region| &region.daemon == daemon)
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
    pub agents: Vec<(PaneKey, AgentState)>,
    /// Each followed daemon: how much of its truth Muster has, and enough about it to decide
    /// deliberately what happens to it.
    pub daemons: Vec<Machine>,
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

pub(crate) fn window() -> WindowNow {
    let session = poison::lock(&SESSION, "session");
    // No reconcile, unlike `publish`. This is a read: a caller asking what the window shows
    // must not be able to move the keyboard or open a region by asking, and anything that
    // needed reconciling has already published.
    let view = session.view();
    let roster = session.roster(&view);
    let numbering = session.numbering(&roster);

    let mut agents = Vec::new();
    let mut daemons = Vec::new();
    for (id, backend) in &session.backends {
        let mirror = poison::lock(&backend.mirror, "mirror");
        let mut directories: Vec<String> = Vec::new();
        let mut panes = 0usize;
        for pane in mirror.panes() {
            let key = PaneKey::new(id, &pane.id);
            let presented = session.attention.presented(&key, pane.agent_state);
            agents.push((key, presented));
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

    WindowNow { view, roster, numbering, agents, daemons }
}

/// Starts following every daemon a config file named.
///
/// No regions yet. Which tab a region shows depends on where the pane in `argv` turned out to
/// live, and that is not known until [`attach`] has asked - so opening one here would mean
/// opening a second one a moment later and closing the first.
///
/// A daemon that will not attach is logged and skipped rather than fatal. One unreachable
/// devenv should cost its own panes and nothing else, and a window that refused to open
/// because a container was down would be worse than the herdr TUI it replaces.
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

/// Reaches one daemon, takes its first snapshot, and starts following it.
fn attach_daemon(daemon: &Daemon) -> Result<(), String> {
    let reached = reach(&daemon.id, &daemon.endpoint)?;
    // Named through the session's registry rather than one of its own, because a name minted
    // reading this snapshot is the name the mirror will hold from here on.
    let names = {
        let session = poison::lock(&SESSION, "session");
        Names::new(daemon.id.clone(), Arc::clone(&session.names), Arc::clone(&session.tab_names))
    };
    let (snapshot, dropped) = fetch_snapshot(&reached.socket_path, &names).map_err(|failure| {
        format!("the daemon {} did not answer at {} ({failure}).", daemon.id, reached.socket_path)
    })?;
    log::info(
        "daemon.attached",
        fields! {
            "daemon" => daemon.id.to_string(),
            "socket" => reached.socket_path.clone(),
            "remote" => reached.tunnel.as_ref().map_or("", Tunnel::host).to_string(),
            "panes" => snapshot.panes.len().to_string(),
            "dropped" => dropped.to_string(),
        },
    );
    let mut session = poison::lock(&SESSION, "session");
    let seeded = session.follow(daemon, reached, snapshot);
    // Before reporting, and explicitly: reporting reads the mirror back through the session
    // and reaches the shell, which may answer by dispatching. Both want this lock.
    drop(session);

    for change in seeded {
        report(&daemon.id, &change);
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
/// What a bare `muster` means. No pane is named, so nothing decides which daemon or which tab
/// beyond what each daemon is already focused on - which is what its user was last looking at
/// and the best answer Muster has to invent.
///
/// Three steps, and each is the reason the next can be simple: be following something, give
/// every daemon a region and a workspace to put in it if it has none, and make a workspace at
/// all costs if that still leaves nothing to show. The last is the one a fresh machine needs,
/// where Muster has just started a daemon that has not answered anything yet - the step above it
/// can only act on a machine that has spoken.
pub(crate) fn open() -> Result<(), String> {
    follow_implicitly_if_nothing_else()?;
    restore_presentation();
    restore_font_sizes();
    reopen_what_was_left();
    // After the file has been read and before anything writes over it. Everything above reads
    // the arrangement; everything from here on is entitled to replace it - which is why this
    // sits above the last two steps rather than below them, and it has to. Asking for a
    // workspace is answered by the daemon on its own thread, and the region for it is opened
    // by the standing rule when that answer lands; a window that had not yet said it was open
    // would turn that rule off and wait forever for a region nothing else will make.
    mark_opened();
    open_remaining_regions();
    open_a_workspace_if_the_window_is_empty();
    publish();
    Ok(())
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

/// Puts back the regions this window was showing when it last closed.
///
/// Before the two rules under it rather than instead of them, which is what makes this an
/// addition and not a special case: a daemon whose saved regions all turned out to be gone
/// falls through to getting a region of its own, and a window where every daemon did falls
/// through to asking for a workspace. So a first launch, a launch after a reboot took
/// everything, and a launch onto a session still running are one path with different amounts
/// of it doing anything.
///
/// Checked against the mirror, which by now holds each attached daemon's snapshot: a saved
/// region is a wish, and a tab nobody holds any more would render as a square that never
/// fills in.
fn reopen_what_was_left() {
    let Some(saved) = saved_arrangement() else { return };

    let mut session = poison::lock(&SESSION, "session");
    let restorable = saved.restorable(|daemon, tab| {
        session
            .backends
            .get(daemon)
            .is_some_and(|backend| poison::lock(&backend.mirror, "mirror").tab(tab).is_some())
    });
    if restorable.regions.is_empty() {
        return;
    }

    let mut restored = Vec::new();
    let mut duplicates = 0usize;
    for region in &restorable.regions {
        // A tab already on screen is shown by the region showing it, and never by a second
        // one beside it. Two regions on one tab render the same pane twice, and only one of
        // the two surfaces can have the terminal - the other prints herdr's refusal and
        // becomes a panel that cannot be closed, because closing it would close the pane the
        // live one is using. A file holding the same region twice is the case that heals
        // here; it is written by a Muster that had already done this once.
        let showing = session.composition.region_showing(&region.daemon, &region.tab);
        if showing.is_some() {
            duplicates += 1;
        }
        let Some(id) =
            showing.or_else(|| session.composition.open_region(&region.daemon, region.tab.clone()))
        else {
            continue;
        };
        session.composition.set_weight(id, region.weight);
        if let Some(pane) = &region.pane {
            session.composition.focus_pane(id, pane.clone());
        }
        restored.push(id);
    }
    if let Some(place) = restorable.focused.and_then(|place| restored.get(place)) {
        session.composition.focus_region(*place);
    }

    log::info(
        "composition.restored",
        fields! {
            "regions" => restored.len().to_string(),
            "dropped" => (saved.regions.len() - restorable.regions.len()).to_string(),
            // Saved regions that named a tab another region was already showing. Anything but
            // zero says the file on disk holds the same tab twice, which is worth knowing
            // rather than healing silently: it is what a window that had drawn a pane twice
            // wrote on its way out, and the count says how many copies it had.
            "duplicates" => duplicates.to_string(),
        },
    );
}

/// Attaches the daemon on this machine when no config file named any.
///
/// Recorded as the wish that produced it - Muster's own daemon, wherever that turns out to be
/// - rather than as the path that answered today.
fn follow_implicitly_if_nothing_else() -> Result<(), String> {
    if following_anything() {
        return Ok(());
    }
    let implicit =
        Daemon { id: DaemonId::new(LOCAL), endpoint: Endpoint::Local { socket_path: None } };
    attach_daemon(&implicit)
}

/// Asks for one workspace when nothing else has produced anything to show.
///
/// This runs during launch, before any daemon has answered a subscription, which is what
/// separates it from [`open_remaining_regions`] beside it: it can tell an empty window from a
/// full one and cannot tell which machine is empty. It has to fire anyway, because a daemon
/// Muster started a moment ago holds nothing and every rule above it produces an empty window.
/// So it picks, and it picks the first local machine: a remote one is somebody else's, and
/// choosing it uninvited is a bigger claim than filling a window. Once a machine has spoken,
/// `open_remaining_regions` asks each one that holds nothing, remote ones included, and no
/// longer has to pick.
///
/// Nothing is opened here. The daemon answers by publishing a workspace, a tab and a pane,
/// and the region appears the way every other region does - through the reconcile that
/// follows. A window that built one itself would be a second place layout is decided.
fn open_a_workspace_if_the_window_is_empty() {
    let empty = {
        let session = poison::lock(&SESSION, "session");
        session.composition.regions().next().is_none()
    };
    if !empty {
        return;
    }

    let Some(daemon) = first_local_daemon() else {
        log::warn(
            "window.empty",
            fields! {
                "impact" => "this window shows nothing yet, because no attached daemon holds a \
                             tab and none of them is on this machine",
                "check" => "nothing, if the remote daemons answer. Each one is asked for a \
                            workspace of its own as soon as it says it holds nothing, which is \
                            a moment after this; what this rule will not do is pick which of \
                            them fills an empty window",
            },
        );
        return;
    };

    // Recorded before the ask rather than after, and in the set the standing rule reads: the
    // machine is asked here, holds nothing until it answers, and the standing rule runs on the
    // very bootstrap that says so. Two records of "already asked" would make a fresh window
    // open with a second workspace nobody wanted.
    {
        let mut session = poison::lock(&SESSION, "session");
        if !session.workspaces_asked_of.insert(daemon.clone()) {
            return;
        }
    }
    ask_for_a_workspace(&daemon);
}

/// Which of a machine's tabs a window somebody asked for may open onto.
///
/// The tab that appeared since it claimed one. `workspace.create` leaves that tab focused, so
/// the daemon's own cursor is the answer almost always, and the scan behind it is for the case
/// where somebody has moved that cursor since.
///
/// Falls back to the cursor when every tab the machine holds is one it already held. That is a
/// claim that was refused, or one whose panes have all been closed since, and showing the
/// machine nothing at all is worse than showing what is there.
fn tab_that_is_ours(mirror: &Mirror, theirs: &BTreeSet<TabId>) -> Option<TabId> {
    let cursor = mirror.focus().tab.clone();
    cursor
        .clone()
        .filter(|tab| !theirs.contains(tab))
        .or_else(|| mirror.tabs().map(|tab| &tab.id).find(|tab| !theirs.contains(tab)).cloned())
        .or(cursor)
}

/// Asks one machine for a workspace, and says what a refusal costs.
///
/// One caller for the empty window and one for a machine holding nothing, so what happens when
/// a daemon says no is written down once.
fn ask_for_a_workspace(daemon: &DaemonId) {
    log::info("workspace.creating", fields! { "daemon" => daemon.to_string() });
    if let Err(refusal) = submit(
        daemon,
        &BackendIntent::CreateWorkspace { cwd: None, run: None, name: None },
        Keyboard::Follows,
    ) {
        log::error(
            "workspace.refused",
            fields! {
                "daemon" => daemon.to_string(),
                "detail" => refusal,
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
/// to ask, which it finds the way herdr's own client would.
pub(crate) fn attach(pane_id: &str) -> Result<Arc<AttachedPane>, AttachError> {
    let pane = PaneId::new(pane_id);
    follow_implicitly_if_nothing_else().map_err(AttachError::Unreachable)?;

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
        let region = match session.composition.region_showing(&daemon, &tab) {
            Some(region) => region,
            None => session
                .composition
                .open_region(&daemon, tab)
                .expect("the daemon holding this pane is one being followed"),
        };
        session.composition.focus_pane(region, pane.clone());
        session.reconcile(&daemon);

        session
            .channel(&daemon, &pane)
            .map(Arc::clone)
            .ok_or_else(|| AttachError::NoChannel("the channel opened and then went".to_string()))?
    };

    // Every other daemon gets a region of its own, on whatever tab it is focused on. This is
    // what puts a laptop and a devenv side by side: `argv` decides where the keyboard starts
    // and the config decides what else is on screen.
    open_remaining_regions();

    mark_opened();
    // Outside the lock, because emitting reaches the shell and a shell reacting to an event
    // by dispatching a request is ordinary.
    publish();
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
/// tab has nothing to inherit from, so herdr would start it in a home directory - and what
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
/// The mirror rather than the registry, because the registry remembers a tab until a prune and
/// the mirror is what the window is actually showing - and a request about a tab that has closed
/// should be refused rather than sent.
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
/// hold it. Two would mean the registry handed one name to two panes, which is a bug in the
/// mint rather than something a caller could have said more precisely - hence the warning
/// rather than a refusal, and the first answer rather than none.
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

/// Gives every daemon with nothing on screen a region of its own, and a workspace if it has
/// nothing to put in one.
///
/// The region goes on the daemon's own focused tab, because that is the one its user was last
/// looking at and Muster has no better answer to invent.
///
/// **A machine holding no tabs at all is asked for a workspace.** Without it, such a machine
/// attaches, appears in the agent list, and cannot be given a pane by anything in the window:
/// every other route to a new pane goes through an existing one, so a machine that reaches zero
/// panes drops out of reach until somebody makes a pane on it with herdr. That is the state a
/// devenv is in the day it is attached, and the state the local machine is in the moment you
/// close its last pane (kan a_2HpkpfIfq).
///
/// Only once the mirror is `Connected`, which is the whole of what separates "this machine says
/// it holds nothing" from "this machine has not spoken yet". `Connected` is set by `bootstrap`
/// and by nothing else, so a daemon still coming up is skipped and picked up by the reconcile
/// behind its first snapshot. Asking one of those would put a workspace on a machine that is
/// full.
///
/// This is the rule that makes a workspace on somebody else's machine, which
/// [`open_a_workspace_if_the_window_is_empty`] deliberately will not do. The two are consistent
/// rather than at odds: that rule *picks* a machine to fill an empty window with, and picking
/// somebody else's is a claim Muster has no business making. This one is told which machine, by
/// a `[[daemon]]` block a person wrote to see that machine's agents - and a machine you asked to
/// see, showing nothing, that nothing can put a pane on, is the bug.
///
/// The ask blocks this thread on a round trip to that daemon, which for a devenv is a round trip
/// over ssh, and this runs on whichever daemon's event thread called the reconcile. Affordable
/// because it is once per machine: `workspaces_asked_of` holds the machine from the moment it is
/// asked, and only a machine that comes back with a tab leaves it.
fn open_remaining_regions() {
    let mut session = poison::lock(&SESSION, "session");
    let showing: Vec<DaemonId> = session.composition.regions().map(|r| r.daemon.clone()).collect();
    let mut wanted: Vec<(DaemonId, TabId)> = Vec::new();
    let mut wants_one: Vec<DaemonId> = Vec::new();
    let mut leave_alone: Vec<DaemonId> = Vec::new();
    let mut inherited: Vec<(DaemonId, BTreeSet<TabId>)> = Vec::new();
    for (id, backend) in &session.backends {
        let mirror = poison::lock(&backend.mirror, "mirror");
        let connected = mirror.health() == Health::Connected;

        // A window somebody asked for, on a machine that has not answered it yet. Whatever
        // that machine last had focused is what the window before this one is showing, and
        // one client may hold a herdr terminal - so none of what it holds is this window's to
        // open onto. What it holds is written down instead, so that the tab this window is
        // about to ask for can be told from the rest, and the ask goes out below.
        if session.fresh && !session.claimed.contains_key(id) && !showing.contains(id) {
            if connected {
                inherited.push((id.clone(), mirror.tabs().map(|tab| tab.id.clone()).collect()));
                wants_one.push(id.clone());
            }
            continue;
        }

        if mirror.tabs().next().is_some() {
            // Left in `workspaces_asked_of` while a claim is outstanding: the machine holds
            // somebody's tabs throughout, so forgetting here would ask it for a workspace
            // again on every event that arrived from anywhere.
            if !session.claimed.contains_key(id) || showing.contains(id) {
                leave_alone.push(id.clone());
            }
        } else if connected {
            wants_one.push(id.clone());
        }
        if showing.contains(id) {
            continue;
        }
        // A claimed machine opens onto the tab that appeared after the claim rather than onto
        // the one it had focused, which is usually the same tab and is not when the claim's
        // answer arrived before anything was waiting for it.
        let opening = match session.claimed.get(id) {
            Some(theirs) => tab_that_is_ours(&mirror, theirs),
            None => mirror.focus().tab.clone(),
        };
        // The mirror has to hold the tab, not only name it: a cursor pointing at a tab this
        // window has not been told the shape of is a region that would render nothing.
        if let Some(tab) = opening
            && mirror.tab(&tab).is_some()
        {
            wanted.push((id.clone(), tab));
        }
    }
    // Forgotten as soon as a machine has a tab, so a machine whose panes all close later is
    // asked again rather than remembered as one Muster has already dealt with.
    for id in leave_alone {
        session.workspaces_asked_of.remove(&id);
    }
    let asking: Vec<DaemonId> =
        wants_one.into_iter().filter(|id| session.workspaces_asked_of.insert(id.clone())).collect();
    // Recorded before the ask goes out, and never removed. A machine is inherited once per
    // window: after the first fill this only says which of its tabs were somebody else's, and
    // preferring the others is what a person opening a window by hand meant either way.
    for (id, theirs) in inherited {
        session.claimed.insert(id, theirs);
    }

    for (daemon, tab) in wanted {
        session.composition.open_region(&daemon, tab);
        session.reconcile(&daemon);
    }
    // Outside the lock, because submitting is a round trip and `submit` takes the session for
    // itself.
    drop(session);
    for daemon in asking {
        ask_for_a_workspace(&daemon);
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

    let file = std::path::PathBuf::from(&*path);
    let staged = file.with_extension("writing");
    let result = std::fs::create_dir_all(file.parent().unwrap_or(std::path::Path::new(".")))
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

/// Writes down what Muster calls each pane and each tab, if it has changed since the last time.
///
/// The same shape as [`save`] beside it, including the staged rename: a window killed
/// mid-write would otherwise come back to a file that parses as far as the third pane and
/// stops, which would strand every pane after it.
///
/// Panes are locked before tabs, and this is the only caller that holds both.
fn save_names(panes: &Arc<Mutex<PaneNames>>, tabs: &Arc<Mutex<TabNames>>) {
    let Some(shared) = shared_names() else { return };

    // The compare first, and outside the hold, because this runs on every publish and a publish
    // follows every agent transition - while the names change only when a pane or a tab appears
    // or goes. Taking a file lock and reading a file for each of those would be a lock per
    // keystroke somebody typed into an agent. What is compared is this window's own last write,
    // so another Muster's change is not missed by it: that is adopted inside the hold, by
    // whichever naming next takes it.
    {
        let mut held = poison::lock(&NAMES_FILE, "saved-names");
        let Some((_, written)) = held.as_mut() else { return };
        let text =
            names::to_toml(&poison::lock(panes, "pane-names"), &poison::lock(tabs, "tab-names"));
        if &text == written {
            return;
        }
        // Cleared rather than set to `text`: what actually lands in the record is decided
        // inside the hold, where another Muster's entries are taken on first, so the text
        // computed here is not what to compare against next time.
        written.clear();
    }

    names::save_shared(panes, tabs, shared.as_ref());

    // What did land, so the next publish can skip the hold.
    let mut held = poison::lock(&NAMES_FILE, "saved-names");
    if let Some((_, written)) = held.as_mut() {
        *written =
            names::to_toml(&poison::lock(panes, "pane-names"), &poison::lock(tabs, "tab-names"));
    }
}

/// The record this window shares, if it has one.
fn shared_names() -> Option<Arc<NamesFile>> {
    poison::lock(&SHARED_NAMES, "shared-names").clone()
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
fn publish() {
    // What the window is showing is also the answer to which agents have been seen, so the
    // two are settled together rather than left to drift. `noticed` is the panes that were
    // waiting to be noticed and have now been - re-announced below, after the shell has been
    // handed the arrangement they appear in.
    let (view, roster, numbering, noticed) = {
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
            // After the reconcile rather than before it, because the reconcile is what would
            // undo it: it resolves every region against the mirror's pane list, so a keyboard
            // put on a pane the mirror has just heard of has to be put there afterwards.
            session.keyboard_to_wanted_pane(daemon);
        }
        let view = session.view();
        let roster = session.roster(&view);
        let numbering = session.numbering(&roster);
        let noticed = session.attention.showing(view.showing().clone());
        // Here because this is the moment composition is settled, and because everything that
        // changes it ends up here - so nothing has to remember to save.
        save(&session.composition, session.presentation, &session.font_sizes);
        session.forget_what_closed();
        save_names(&session.names, &session.tab_names);
        (view, roster, numbering, noticed)
    };

    // The shape, not the fact. "the view changed" is useless in a bug report and what it
    // changed to is the whole answer - a window rendering the wrong thing and a window
    // rendering nothing are one line apart here (`architecture.md`, the diagnostic log).
    for region in &view.regions {
        log::info(
            "view.region",
            fields! {
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
    log::info(
        "roster.published",
        fields! {
            "tabs" => roster.tabs().count().to_string(),
            "tabs_on_screen" => roster.tabs().filter(|tab| tab.on_screen).count().to_string(),
            "panes" => roster.panes().count().to_string(),
            "on_screen" => roster.panes().filter(|pane| pane.on_screen).count().to_string(),
            "numbering" => describe_numbering(&numbering),
        },
    );
    ffi::emit(&Event { payload: Some(event::Payload::ViewChanged(convert::view(&view))) });
    ffi::emit(&Event {
        payload: Some(event::Payload::RosterChanged(convert::roster(&roster, &numbering))),
    });

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
    let showed = {
        let mut session = poison::lock(&SESSION, "session");
        session.reconcile(daemon);
        // A tab Muster asked for, now that the daemon has said what is in it. Here rather
        // than at the moment it was asked for, because until this event a region showing it
        // would be a region showing a tab the mirror has never heard of.
        session.show_wanted_tab(daemon)
    };
    // A standing rule rather than a launch-time one, because the states that produce a
    // daemon with nothing on screen keep arriving: a workspace Muster asked for a moment ago
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
        open_remaining_regions();
    }
    if showed {
        publish();
    }
}

/// Turns what one daemon said into a log line and, where the window renders it, an event.
///
/// The whole of D's answer to "agent states are the point": every pane's transitions reach
/// the log, and the attached one's reaches the chrome. Which pane the window is showing is
/// the shell's business, so every pane is sent and the shell decides.
fn announce(daemon: &DaemonId, notice: Notice) {
    match notice {
        Notice::Bootstrapped { changes, dropped, denied_tabs } => {
            log::info(
                "mirror.bootstrap",
                fields! {
                    "daemon" => daemon.to_string(),
                    "changes" => changes.len().to_string(),
                    "dropped" => dropped.to_string(),
                    "denied_tabs" => denied_tabs.to_string(),
                },
            );
            if denied_tabs > 0 {
                log::warn(
                    "mirror.tabs_denied",
                    fields! {
                        "daemon" => daemon.to_string(),
                        "count" => denied_tabs.to_string(),
                        "impact" => "this daemon's snapshot disagreed with itself and the tabs \
                                     it could not account for are not drawn. Right where the \
                                     daemon is holding tabs no pane is in; wrong if it holds \
                                     panes this snapshot left out, and then those panes are \
                                     missing from the window too",
                        "check" => "ask this daemon `herdr api snapshot` and compare its tabs \
                                    with its panes. A tab no pane names is one herdr has \
                                    already closed and not announced \
                                    (observations/herdr-0.8.0.md section 15)",
                    },
                );
            }
            if dropped > 0 {
                log::warn(
                    "mirror.entries_dropped",
                    fields! {
                        "daemon" => daemon.to_string(),
                        "count" => dropped.to_string(),
                        "impact" => "the session renders with fewer panes than the daemon \
                                     holds, which looks like panes the user closed",
                        "check" => "a herdr whose pane, tab or workspace payload has moved - \
                                    compare corpus/herdr-<version>/api-schema.json",
                    },
                );
            }
            // A bootstrap replaces the whole picture, so anything composition names may have
            // gone in the gap it was rebuilt across.
            reconcile(daemon);
            publish();
            health(daemon, "connected", "");
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
                publish();
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
            health(daemon, "stale", &detail);
        }
        Notice::Reconnected => {
            log::info("backend.reconnected", fields! { "daemon" => daemon.to_string() });
            health(daemon, "connected", "");
        }
        Notice::UnknownEvent { kind } => log::warn(
            "backend.unknown_event",
            fields! {
                "daemon" => daemon.to_string(),
                "kind" => kind,
                "impact" => "whatever this event reports is not reaching the mirror, so the \
                             view is missing that kind of change entirely",
                "check" => "whether this herdr is newer than the pinned one - if the event \
                            matters, it needs reading in muster-herdr's decoder",
            },
        ),
    }
}

fn report(daemon: &DaemonId, change: &Change) {
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

    // The one change that is about an interval rather than a pane, and the only evidence
    // there will ever be that something happened while Muster was not listening. herdr's
    // counter is session-wide and reaches a client only in a snapshot, so nothing says
    // which panes moved or what they moved to - and after this line, nothing ever can.
    if let Change::AgentTransitionsMissed { expected, saw } = change {
        log::warn(
            "agent.transitions_missed",
            fields! {
                "daemon" => daemon.to_string(),
                "expected" => expected.to_string(),
                "saw" => saw.to_string(),
                "missed" => saw.saturating_sub(*expected).to_string(),
                "impact" => "the states shown are right, but an agent may have asked for a \
                             person while this window was disconnected and gone back to work \
                             since - so a request for attention was never routed",
                "check" => "the backend.stale record above this, for how long the gap was, and \
                            whether any pane on this daemon is waiting on somebody",
            },
        );
    }

    // Recorded before anything is announced, because it is what the announcement depends on:
    // whether this transition finished on a pane somebody was looking at is the difference
    // between `idle` and `done`.
    //
    // The lock is let go before anything is emitted, on the same terms as `announce_state`
    // below: emitting reaches the shell, the shell reacts by dispatching, and a dispatch
    // arriving while this held the session would deadlock against it on the same thread.
    let attended = match change {
        Change::AgentStateChanged { pane, from, to } => {
            let key = PaneKey::new(daemon, pane);
            let attended = poison::lock(&SESSION, "session").attention.observed(&key, *from, *to);
            attended.map(|attend| (key, attend))
        }
        // A pane that was already finished when this window arrived. Muster saw no transition
        // for it and the daemon did, so first sight takes the daemon's answer; everything
        // after it is Muster's own (`muster_core::attention`).
        //
        // No notification for one, and that is deliberate: this is a pane that finished before
        // the window existed, so a banner would be Muster announcing history at launch. The
        // roster and the border say it, which is what they are for.
        Change::PaneAdded(pane) => {
            let key = PaneKey::new(daemon, pane);
            let mut session = poison::lock(&SESSION, "session");
            if let Some(backend) = session.agent_state(&key) {
                session.attention.first_seen(&key, backend);
            }
            None
        }
        // Both spellings of removal, because both mean the pane is gone: one is a client
        // closing it and the other is its program ending (`architecture.md`, event model).
        Change::PaneRemoved { pane, .. } => {
            let key = PaneKey::new(daemon, pane);
            let attended = poison::lock(&SESSION, "session").attention.forget(&key);
            attended.map(|attend| (key, attend))
        }
        _ => None,
    };
    if let Some((pane, attend)) = attended {
        announce_attention(&pane, attend);
    }

    if let Some(pane) = change.announces_agent_state() {
        announce_state(&PaneKey::new(daemon, pane));
    }
}

/// Tells the shell that a pane has started asking for somebody, or stopped.
///
/// The label travels with it rather than being looked up by the shell, because naming a pane
/// is the core's decision (`roster`) and a banner naming an agent differently from the row it
/// appears on is two names for one thing.
fn announce_attention(pane: &PaneKey, attend: Attend) {
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
    let Some(state) = presented(pane) else { return };
    ffi::emit(&Event {
        payload: Some(event::Payload::PaneStateChanged(PaneStateChanged {
            daemon_id: pane.daemon.to_string(),
            pane_id: pane.pane.to_string(),
            state: state.as_str().to_string(),
        })),
    });
}

/// What the window should show for a pane, which is not always what the daemon said.
///
/// The mirror is read back rather than the change being taken at its word, because one of
/// the two changes that announce a pane carries no state at all - a pane that appears
/// already running is the case that needs this. For a transition the mirror was written
/// before this runs, so it holds exactly what the transition moved to.
///
/// Then `done` is decided here rather than accepted from the daemon, because the daemon
/// cannot see this window (`attention`).
fn presented(pane: &PaneKey) -> Option<AgentState> {
    let session = poison::lock(&SESSION, "session");
    let backend = session.agent_state(pane)?;
    Some(session.attention.presented(pane, backend))
}

/// The window gained or lost the OS's focus.
///
/// The one thing about attention no daemon can tell the core and no core can observe. What
/// it changes is which finished agents are still waiting to be noticed, so only those panes
/// are re-announced - an agent-state change costs that change rather than a walk of every
/// pane (`architecture.md`, fast is a feature).
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

/// What a search is showing, which is everything the shell draws in the find bar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Findings {
    pub(crate) total: u32,
    /// Which hit is selected, counting from one, or zero when nothing matched.
    ///
    /// Counting from one because it is a number a person reads - "3 of 47" - and zero is
    /// already the answer to "which of nothing".
    pub(crate) selected: u32,
    pub(crate) rows_searched: u32,
    pub(crate) truncated: bool,
}

/// Looks for something in the pane with the keyboard, and lands on the first match.
///
/// A read and a scroll, in that order, and the whole of what typing in the find bar does.
/// An empty needle answers without asking anybody: it means nothing has been typed yet, and
/// a round trip per keystroke on the way to an empty field would be a round trip for nothing.
pub(crate) fn find(daemon: &DaemonId, pane: &PaneId, needle: &Needle) -> Result<Findings, String> {
    if needle.is_empty() {
        end_find();
        return Ok(Findings::default());
    }
    let channel = channel(daemon)?;
    let found = channel.find(pane, needle).map_err(|refusal| {
        format!(
            "the daemon {daemon} would not read pane {pane}, so nothing was searched: {refusal}"
        )
    })?;

    let selected = (!found.hits.is_empty()).then_some(0);
    let search = Search { daemon: daemon.clone(), pane: pane.clone(), found, selected };
    let findings = search.findings();
    land(&search);
    poison::lock(&SESSION, "session").search = Some(search);
    Ok(findings)
}

/// Moves to the next match, or the previous one, and lands on it.
///
/// Wraps at both ends, because a list somebody is stepping through has no reason to stop
/// having reached one end of it - and the alternative is a chord that silently does nothing.
pub(crate) fn step_find(forward: bool) -> Result<Findings, String> {
    let mut session = poison::lock(&SESSION, "session");
    let Some(search) = session.search.as_mut() else {
        return Err("nothing is being searched for, so there was no next match to go to. The \
                    find bar sends this, so either it is open with nothing typed in it or the \
                    core forgot a search the shell still has on screen."
            .to_string());
    };
    let total = search.found.hits.len();
    if total == 0 {
        return Ok(search.findings());
    }
    search.selected = Some(match (search.selected, forward) {
        (Some(at), true) => (at + 1) % total,
        (Some(at), false) => (at + total - 1) % total,
        (None, _) => 0,
    });
    let findings = search.findings();
    // Taken out from under the lock, because landing is a round trip and holding the session
    // across one stalls every event arriving from every other daemon.
    let landing = Search {
        daemon: search.daemon.clone(),
        pane: search.pane.clone(),
        found: search.found.clone(),
        selected: search.selected,
    };
    drop(session);
    land(&landing);
    Ok(findings)
}

/// Forgets the search, which is what closing the find bar means.
pub(crate) fn end_find() {
    poison::lock(&SESSION, "session").search = None;
}

impl Search {
    fn findings(&self) -> Findings {
        Findings {
            total: u32::try_from(self.found.hits.len()).unwrap_or(u32::MAX),
            selected: self.selected.map_or(0, |at| u32::try_from(at + 1).unwrap_or(u32::MAX)),
            rows_searched: self.found.rows_searched,
            truncated: self.found.truncated,
        }
    }
}

/// Puts the selected match on screen.
///
/// Two requests on two channels, because herdr scrolls by steps rather than to a place: where
/// the pane is looking has to be asked for, and the difference is what gets sent
/// (`observations/herdr-0.8.0.md` section 17). A pane already showing the match is left alone,
/// so stepping through hits on one screen does not jog the view under somebody reading it.
///
/// Nothing here fails loudly. The count is already right and already on screen; a landing that
/// did not happen costs a scroll somebody can do themselves, and a refusal per keystroke in the
/// log would bury the one that mattered.
fn land(search: &Search) {
    let Some(hit) = search.selected.and_then(|at| search.found.hits.get(at)) else {
        return;
    };
    let Ok(channel) = channel(&search.daemon) else {
        return;
    };
    let Ok(viewport) = channel.viewport(&search.pane) else {
        return;
    };
    if viewport.shows(hit.rows_from_bottom) {
        return;
    }
    let Some(attached) = attached_pane(&search.daemon, &search.pane) else {
        // A pane with no bridge is one no region is showing, which a find bar over the
        // focused pane cannot be about. Worth a line rather than silence, because it means
        // the window and the session disagree about what has the keyboard.
        log::debug(
            "find.unattached",
            fields! {
                "daemon" => search.daemon.to_string(),
                "pane" => search.pane.to_string(),
                "impact" => "the match was found and counted, and the pane did not move to it.",
            },
        );
        return;
    };

    let wanted = viewport.centred_on(hit.rows_from_bottom);
    let (direction, rows) = if wanted > viewport.rows_from_bottom {
        (ScrollDirection::Up, wanted - viewport.rows_from_bottom)
    } else {
        (ScrollDirection::Down, viewport.rows_from_bottom - wanted)
    };
    if rows > 0 {
        attached.input.scroll(direction, u16::try_from(rows).unwrap_or(u16::MAX));
    }
}

/// Reads a pane back, and changes nothing.
///
/// A round trip at the moment somebody asks, like [`find`] above and for the same reason: a
/// pane's output never enters the core, so there is nothing held here to answer from. The
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

/// Writes what a pane should be again, and asks the daemons Muster owns to read it.
///
/// The counterpart of [`reset_pane_input`] for the settings Muster does not act on itself. It
/// cannot reach as far: a pane's shell and its scrollback limit are arguments herdr takes when
/// it builds a pane's terminal, so a reload reaches panes opened afterwards and no others -
/// the same honest limit `pane_padding` already carries. What it does reach immediately is the
/// update checks, which is why a daemon started before the file changed still gets asked.
///
/// Silent when nothing moved. Most reloads change a colour, and a file that says what it
/// already said is a request no daemon needs.
pub(crate) fn rewrite_daemon_configuration() {
    let Some((_, changed)) = write_daemon_configuration() else { return };
    if !changed {
        return;
    }
    let text = daemon_configuration_text();

    let told: Vec<(String, Option<(Remote, String)>)> = {
        let session = poison::lock(&SESSION, "session");
        session
            .backends
            .values()
            .filter(|backend| backend.owns_config)
            .map(|backend| {
                let over_there = backend
                    .tunnel
                    .as_ref()
                    .zip(backend.remote_config.as_ref())
                    .map(|(tunnel, path)| (tunnel.remote(), path.clone()));
                (backend.socket_path.clone(), over_there)
            })
            .collect()
    };
    // Outside the lock: these are round trips to other processes, and on other machines, and
    // holding the session through one would stop every event this window is listening for.
    for (socket, over_there) in &told {
        // The far machine's copy first, for the reason the local one is written before the
        // daemon starts: a daemon asked to re-read a file that still says the old thing reads
        // the old thing, and reports success doing it.
        if let Some((remote, path)) = over_there
            && let Err(detail) = remote.place(path, text.as_bytes(), "0644")
        {
            log::warn(
                "daemon.config.unsent",
                fields! {
                    "host" => remote.host(),
                    "path" => path.clone(),
                    "detail" => detail,
                    "impact" => "that machine's panes keep the settings its daemon was started \
                                 with, so a shell or a scrollback depth changed just now \
                                 applies to the panes beside them and not to those",
                    "check" => "whether the connection to that host is still up - the run log \
                                says tunnel.down if it is not",
                },
            );
            continue;
        }
        daemon::reload_configuration(socket);
    }
}

/// Points every attached pane at typing settings that have just been read again.
///
/// Every pane or none, which is the whole reason this exists rather than only setting the
/// static: a reload that reached the static alone would take effect on panes opened afterwards
/// and leave the rest as they were, so what `option_as_alt` meant would depend on when each
/// pane happened to be opened.
///
/// A pane whose encoder will not build keeps the one it had. That is the better of two bad
/// answers - the alternative is a pane that stops typing - and it is loud, because the pane it
/// happens to is named.
pub(crate) fn reset_pane_input(settings: &PaneInputSettings) {
    set_pane_input(settings.clone());

    let panes: Vec<(DaemonId, PaneId, Arc<AttachedPane>)> = {
        let session = poison::lock(&SESSION, "session");
        session
            .panes
            .iter()
            .flat_map(|(daemon, panes)| {
                panes
                    .iter()
                    .map(move |(pane, held)| (daemon.clone(), pane.clone(), Arc::clone(held)))
            })
            .collect()
    };

    let mut resettled = 0usize;
    for (daemon, pane, held) in &panes {
        match KeyEncoder::new(settings.profile()) {
            Ok(encoder) => {
                held.input.resettle(Arc::new(encoder), settings);
                resettled += 1;
            }
            Err(error) => log::warn(
                "config.reload.encoder",
                fields! {
                    "daemon" => daemon.to_string(),
                    "pane" => pane.to_string(),
                    "detail" => error.to_string(),
                    "impact" => "this pane keeps the typing settings it was attached with, so it \
                                  now disagrees with the rest of the window about what option \
                                  means",
                    "check" => "libghostty-vt is behind this; a relaunch rebuilds every encoder \
                                 from scratch",
                },
            ),
        }
    }
    log::info("config.reload.typing", fields! { "panes" => resettled.to_string() });
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
    publish();
    Ok(())
}

fn no_pane_to_size() -> String {
    "no pane has this window's keyboard, so there was no text to size. Text size is per pane \
     now, and this chord means the pane in front of you - the attach failed earlier, or the \
     pane it succeeded on exited."
        .to_string()
}

/// Hands every attached pane back at the size its daemon lays it out at.
///
/// The shell's word that this process is going away, acted on before its bridges are killed.
/// A controlling client holds a pane's terminal at its own geometry and herdr does not release
/// that on detach, so a Muster that quits leaves every pane it touched sized to a window that
/// no longer exists - and the herdr TUI somebody opens next inherits it
/// (`observations/herdr-0.8.0.md` section 4).
///
/// Not the size the pane had before Muster arrived, which nothing can answer: herdr publishes a
/// pane's rows and never its columns. The daemon's own layout is the answer instead, and it is
/// the one that matters - what is about to draw these panes is herdr itself.
///
/// Waited for rather than fired and forgotten. The resize leaves over the pane's control
/// socket, is relayed by a bridge this process is about to kill, and lands in the daemon a
/// moment later; without the wait, quitting races the thing it is trying to do. `viewport_rows`
/// is what the daemon will say back - the one dimension it reports - so that is what is
/// watched.
///
/// Or ends them, if that is what was asked. Handing a pane back at a tidy size is care taken
/// over a session somebody is coming back to, and there is no session to come back to here -
/// so that half is skipped rather than done and then undone.
pub(crate) fn quitting(close_sessions: bool) {
    let daemons: Vec<(DaemonId, String, Names)> = {
        let session = poison::lock(&SESSION, "session");
        session
            .backends
            .iter()
            .map(|(id, backend)| (id.clone(), backend.socket_path.clone(), backend.names.clone()))
            .collect()
    };

    if close_sessions {
        close_daemons(&daemons);
        return;
    }

    let mut unreachable = 0usize;
    let mut wanted: Vec<(DaemonId, PaneId, u16)> = Vec::new();
    for (daemon, socket_path, names) in &daemons {
        // Outside the session lock: this is a round trip, and a wedged daemon must not hold up
        // the ones that are answering while somebody is waiting to quit.
        let sizes = match muster_herdr::fetch_unattached_sizes(socket_path, names) {
            Ok(sizes) => sizes,
            Err(failure) => {
                log::warn(
                    "quit.geometry.unknown",
                    fields! {
                        "daemon" => daemon.to_string(),
                        "detail" => failure.to_string(),
                        "impact" => "this daemon's panes keep the size this window gave them, so \
                                     a terminal opened on them next renders into a grid the \
                                     wrong shape until something resizes it",
                        "check" => "whether that daemon is still answering at all - it is being \
                                    asked one last question on the way out",
                    },
                );
                continue;
            }
        };

        let panes: Vec<(PaneId, Arc<AttachedPane>)> = {
            let session = poison::lock(&SESSION, "session");
            session
                .panes
                .get(daemon)
                .into_iter()
                .flatten()
                .map(|(pane, held)| (pane.clone(), Arc::clone(held)))
                .collect()
        };
        for (pane, held) in panes {
            // Only panes this window is driving. A pane it never attached to was never held at
            // Muster's geometry, and resizing one would be moving something Muster did not move.
            let Some(cells) = sizes.get(&pane) else { continue };
            if held.input.resize(cells.columns, cells.rows) {
                wanted.push((daemon.clone(), pane, cells.rows));
            } else {
                // A pane whose bridge has already gone. Its hold is whatever that bridge left,
                // and there is no longer a route to it - this window's only way onto a pane's
                // terminal is the stream its bridge holds open.
                unreachable += 1;
            }
        }
    }

    let settled = wait_for_geometry(&daemons, &wanted);
    let missed = wanted.len() - settled + unreachable;
    log::info(
        "quit.geometry.restored",
        fields! {
            "asked" => wanted.len().to_string(),
            "settled" => settled.to_string(),
            "unreachable" => unreachable.to_string(),
            "impact" => if missed == 0 {
                String::new()
            } else {
                format!(
                    "{missed} pane(s) keep the size this window gave them, so a terminal opened \
                     on them renders into a grid the wrong shape until something resizes it"
                )
            },
        },
    );
}

/// Asks every daemon this window is attached to stop, and says what came of it.
///
/// `server.stop` rather than a signal, because a signal is not available: Muster puts a daemon
/// it starts in a process group of its own so that quitting cannot take the agents with it, and
/// a daemon opened through Launch Services has no pid here at all. The socket is the only
/// handle, and it is the better one - measured, it gives a pane's process a catchable SIGHUP
/// and a window to act in rather than a SIGKILL (kan a_28YghIUw2).
///
/// A longer timeout than an ordinary request. This is a daemon tearing down every pane it
/// holds, and the alternative to waiting is reporting a failure for a stop that worked.
///
/// A daemon that will not answer is reported and left. It is still running, its agents are
/// still going, and the honest thing is to say which one rather than to insist - `muster
/// window` names every daemon and its socket, which is what makes ending one by hand safe.
fn close_daemons(daemons: &[(DaemonId, String, Names)]) {
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);
    let (mut stopped, mut refused) = (0usize, Vec::new());
    for (daemon, socket_path, _) in daemons {
        match daemon::stop(socket_path, PATIENCE) {
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
                        "check" => "whether that daemon is answering at all - `muster window` \
                                    names its socket, and `HERDR_SOCKET_PATH=<socket> herdr \
                                    server stop` ends it by hand",
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

/// Waits until the daemons agree that the panes are the size they were asked to be.
///
/// Rows, because rows are what herdr reports back: a pane's `viewport_rows` is the one
/// dimension in its payload, and columns are in none of it. Half an oracle answers the question
/// that is actually being asked here, which is whether the message arrived at all.
///
/// Bounded, and short. This is between somebody pressing ⌘Q and the window going, so the honest
/// trade is a moment of delay against a pane handed back wrong - and a daemon that has stopped
/// answering has already been reported by the fetch above.
fn wait_for_geometry(
    daemons: &[(DaemonId, String, Names)],
    wanted: &[(DaemonId, PaneId, u16)],
) -> usize {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    let mut settled = 0;
    let mut waiting: Vec<&(DaemonId, PaneId, u16)> = wanted.iter().collect();
    while !waiting.is_empty() && std::time::Instant::now() < deadline {
        waiting.retain(|(daemon, pane, rows)| {
            let Some((_, socket_path, names)) = daemons.iter().find(|(id, ..)| id == daemon) else {
                return false;
            };
            match muster_herdr::pane_rows(socket_path, names, pane) {
                Some(reported) if reported == *rows => {
                    settled += 1;
                    false
                }
                _ => true,
            }
        });
        if !waiting.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    settled
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

pub(crate) fn window_focused(focused: bool) {
    log::info("window.focus", fields! { "focused" => focused });
    let noticed = {
        let mut session = poison::lock(&SESSION, "session");
        session.attention.window_focused(focused)
    };
    for pane in &noticed.settled {
        announce_state(pane);
    }
    // A pane somebody has just looked at is not asking any more, whatever its state. That is
    // the same rule the border already follows and the reason a focused window showing a pane
    // never raised one in the first place.
    for pane in &noticed.withdrawn {
        announce_attention(pane, Attend::Withdrawn);
    }
}

/// How much of one daemon's truth the core currently has.
///
/// Per daemon, because health is per connection. A window showing a laptop and a devenv has
/// two answers, and one of them going stale says nothing about the other - so a single
/// window-wide state would let a dropped VPN read as though every session had gone.
fn health(daemon: &DaemonId, state: &str, detail: &str) {
    ffi::emit(&Event {
        payload: Some(event::Payload::BackendHealth(BackendHealth {
            daemon_id: daemon.to_string(),
            state: state.to_string(),
            detail: detail.to_string(),
        })),
    });
}

/// The moment the pane becomes typeable, on the thread that accepted the connection.
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
