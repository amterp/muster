//! What the shell hands the core once, at launch, and what the config file last resolved to.
//!
//! Where things are - the daemon binary and its data, the Linux daemons, the commands
//! directory, the daemon records, the config file - and the platform's locale are answers only
//! the shell has. The bindings, typing settings, root knobs, appearance and `[[daemon]]` blocks
//! are the config file's, set at launch and again on each reload. Each sits in a process-wide
//! cell with a setter and a getter, because the code that needs one is several calls away from
//! anything holding a config file.
//!
//! None of it is a live session. A [`super::Session`] holds what is attached and running, and
//! [`super::reset`] replaces it; these cells describe the process the session runs in, and the
//! session reads them as it attaches daemons and panes. A setting the session has to push to
//! every daemon it follows when it changes stays beside the session, in `session.rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use muster_core::composition::{Daemon, DaemonId};
use muster_core::config::{Appearance, Feel, Rgb};
use muster_core::diagnostics::poison;
use muster_core::input::{Bindings, PaneInputSettings};
use muster_daemon_client::install as remote_install;

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

pub(super) fn daemon_binary() -> Option<String> {
    poison::lock(&DAEMON_BINARY, "daemon-binary").clone()
}

/// The daemon's data directory, when the shell keeps it somewhere other than beside the
/// binary. None is the directory beside it, which is where a build puts it.
pub(super) static DAEMON_DATA: Mutex<Option<String>> = Mutex::new(None);

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
pub(super) fn carried() -> remote_install::Carried {
    let mac = daemon_binary().map(PathBuf::from);
    let data = poison::lock(&DAEMON_DATA, "daemon-data").clone().map(PathBuf::from).or_else(|| {
        mac.as_deref().and_then(Path::parent).map(|beside| beside.join("muster-daemon-data"))
    });
    // The CLI the app pointed `muster` in the commands directory at when it started, which is
    // the one that belongs to this build.
    let mac_cli = commands_path()
        .and_then(|commands| Path::new(&commands).join("muster").canonicalize().ok());
    let mac_here = cfg!(all(target_os = "macos", target_arch = "aarch64"));
    remote_install::Carried {
        linux: poison::lock(&REMOTE_DAEMONS, "remote-daemons").clone().map(PathBuf::from),
        extras: mac_cli.as_deref().map(muster_core::harnesses::extras_beside),
        mac: mac.filter(|_| mac_here),
        mac_cli: mac_cli.filter(|_| mac_here),
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

pub(super) fn platform_locale() -> Option<String> {
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

pub(super) fn commands_path() -> Option<String> {
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
    *poison::lock(&MACHINE_COLORS, "settings") = appearance.machine_colors.clone();
    *poison::lock(&APPEARANCE, "settings") = Some(appearance);
}

/// The colors the config file chose for machines' marks, apart from the rest of the
/// appearance because every roster published reads them, and cloning a palette and a font
/// name to get at them would be work per publish for nothing.
static MACHINE_COLORS: Mutex<BTreeMap<DaemonId, Rgb>> = Mutex::new(BTreeMap::new());

pub(crate) fn machine_colors() -> BTreeMap<DaemonId, Rgb> {
    poison::lock(&MACHINE_COLORS, "settings").clone()
}

/// The appearance in force, which with no config file is every value absent - so the renderer
/// paints what it would have painted anyway.
pub(crate) fn appearance() -> Appearance {
    poison::lock(&APPEARANCE, "settings").clone().unwrap_or_default()
}

/// The `[[daemon]]` blocks the running configuration was built from.
///
/// Held separately from what is attached, because those are different questions and only one of
/// them is about the file. A config naming no daemons still ends up with one attached - Muster
/// starts its own when nothing answers - so comparing a new file against what is attached would
/// report a change on every reload of a file that never mentioned a daemon at all.
pub(super) static CONFIGURED_DAEMONS: Mutex<Option<Vec<Daemon>>> = Mutex::new(None);

pub(crate) fn set_configured_daemons(daemons: &[Daemon]) {
    *poison::lock(&CONFIGURED_DAEMONS, "settings") = Some(daemons.to_vec());
}

/// Clears what a [] starts over from, so the next test's launch sets its own.
pub(super) fn forget() {
    *poison::lock(&DAEMON_BINARY, "daemon-binary") = None;
    *poison::lock(&PLATFORM_LOCALE, "locale") = None;
    *poison::lock(&COMMANDS, "commands") = None;
    *poison::lock(&BINDINGS, "bindings") = None;
    *poison::lock(&PANE_INPUT, "input-settings") = None;
    *poison::lock(&FEEL, "settings") = None;
    *poison::lock(&CONFIG_PATH, "settings") = None;
    *poison::lock(&APPEARANCE, "settings") = None;
    *poison::lock(&CONFIGURED_DAEMONS, "settings") = None;
}
