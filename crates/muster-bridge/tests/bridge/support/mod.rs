//! A whole Muster, assembled from real parts, so a test can type into it.
//!
//! Every binary here does the same expensive setup: a scratch daemon, a config pointing the
//! core at it, a window with a pane in it, a real bridge drawing that pane, and a way to read
//! what a surface would be showing. What differs between them is the keystrokes and the config,
//! which is the part worth reading in the test itself.
//!
//! The event callback below writes into process globals, and a `Startup` installs the run log
//! for the whole process, so tests sharing a process take turns: a [`Typing`] holds the seam's
//! turn (`muster::testing::fresh_session`) for as long as it lives, and clears this file's own
//! record of the window when it starts.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use muster::proto::{
    AttachPane, Attached, Event, KeyDown, KeyEvent, OpenWindow, Request, Response, Startup,
    ViewChanged, ViewNode, event, request, response, view_node,
};
use muster::testing::Turn;
use muster_daemon_proto::input_event;
use muster_harness::{Daemon, Input, until_some};
use muster_vt::{Grid, Terminal};
use prost::Message;

/// The bridge falls back to this when its stdout is a pipe rather than a surface's PTY, so
/// the oracle reads the pane at the size the daemon is rendering it for.
pub(crate) const COLUMNS: u16 = 80;
pub(crate) const ROWS: u16 = 24;

/// Set from the callback the shell would register, which is also the only test of the push
/// direction: the pane becoming typeable is announced, not answered.
static TYPEABLE: AtomicBool = AtomicBool::new(false);

extern "C" fn note_events(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    match Event::decode(bytes).ok().and_then(|event| event.payload) {
        Some(event::Payload::PaneTypeable(_)) => TYPEABLE.store(true, Ordering::Relaxed),
        // Where a shell learns which panes are on screen and how many times each one's bridge
        // has been replaced.
        Some(event::Payload::ViewChanged(changed)) => record_panes(&changed),
        _ => {}
    }
}

/// Every pane on screen, by the name Muster gave it, with how many replacements its bridge has
/// had.
///
/// The count is the whole of what the shell is told about a bridge that died - a number that
/// moved is what makes a window build a new surface, and building one is the only way a bridge
/// is ever started. Replaced whole on every view rather than merged, so a pane that has gone
/// stops being answerable here at the same moment it leaves the window.
static ON_SCREEN: Mutex<BTreeMap<String, u32>> = Mutex::new(BTreeMap::new());

/// How many times the last published view says this pane's bridge has been replaced.
pub(crate) fn restarts(pane: &str) -> Option<u32> {
    poison_free(&ON_SCREEN).get(pane).copied()
}

fn record_panes(view: &ViewChanged) {
    fn walk(node: &ViewNode, into: &mut BTreeMap<String, u32>) {
        match node.node.as_ref() {
            Some(view_node::Node::Pane(pane)) => {
                into.insert(pane.pane_id.clone(), pane.bridge_restarts);
            }
            Some(view_node::Node::Split(split)) => {
                for child in [split.first.as_deref(), split.second.as_deref()].into_iter().flatten()
                {
                    walk(child, into);
                }
            }
            None => {}
        }
    }

    let mut seen = BTreeMap::new();
    for root in view.regions.iter().filter_map(|region| region.root.as_ref()) {
        walk(root, &mut seen);
    }
    // An empty view is a window between arrangements, and forgetting every pane because of one
    // would make a lookup fail for a pane that is still there.
    if !seen.is_empty() {
        *poison_free(&ON_SCREEN) = seen;
    }
}

fn poison_free<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Everything between a fresh daemon and a pane that can be typed into.
///
/// `config` is whatever the test wants in the file beyond the `[[daemon]]` block naming this
/// daemon - which is how a config-driven behavior gets exercised through the path a person
/// uses, rather than by reaching past the file into the core.
///
/// Fields drop in order, and the order is deliberate: the bridge goes before the daemon it is
/// drawing from, and the seam's turn is given up last.
pub(crate) struct Typing {
    pub(crate) bridge: Bridge,
    pub(crate) daemon: Daemon,
    /// What Muster calls the pane, which is also what its daemon calls it.
    pub(crate) pane: String,
    /// The core's run log, where a test reads what the core decided when no event says so.
    pub(crate) log: PathBuf,
    _turn: Turn,
}

impl Typing {
    pub(crate) fn start(config: &str) -> Typing {
        let turn = muster::testing::fresh_session();
        // This file's own record of the window, which the seam's reset does not reach.
        TYPEABLE.store(false, Ordering::Relaxed);
        poison_free(&ON_SCREEN).clear();

        let daemon = Daemon::start_built();

        // A config file naming this daemon's socket, which is how a person points Muster at
        // a daemon it did not start.
        let path = daemon.muster_config_with(config);
        let log = daemon.root().join("core.log");
        // Registered before anything can publish, because the first view names the pane.
        muster::ffi::muster_set_event_callback(Some(note_events));
        assert_ok(&answer(request::Payload::Startup(Startup {
            config_path: path.to_string_lossy().into_owned(),
            log_path: log.to_string_lossy().into_owned(),
            ..Startup::default()
        })));
        // A window with nothing in it asks its daemon for a tab, which is where the pane
        // comes from: this test makes none of its own.
        assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
        let pane = until_some("the window to show the pane it asked its daemon for", || {
            let on_screen = poison_free(&ON_SCREEN);
            (on_screen.len() == 1).then(|| on_screen.keys().next().cloned()).flatten()
        });

        let attached = attach_or_explain(&pane, &path);
        let bridge = Bridge::spawn(&pane, daemon.socket_path(), &attached.link_socket_path);
        until(
            "the bridge to tell the window it attached",
            || TYPEABLE.load(Ordering::Relaxed),
            || bridge.diagnosis("nothing became typeable"),
        );
        Typing { bridge, daemon, pane, log, _turn: turn }
    }

    /// Runs a command in the pane, and waits until nothing but it will read what is typed.
    ///
    /// Typed on an input connection of the test's own rather than through the path under
    /// test, so that a broken input path fails at the assertion rather than at the
    /// arrangement. A marker the shell prints first says the line was read and is being run:
    /// from then on the shell reads nothing more from the terminal, so a keystroke sent after
    /// it waits in the terminal for the command even if the command has not quite started.
    /// The marker is spelled as arithmetic so that the shell echoing the line back is not
    /// mistaken for the shell running it.
    pub(crate) fn run(&self, command: &str) {
        let mut input = Input::connect(self.daemon.socket_path());
        input.send(
            &self.pane,
            input_event::Input::Send(input_event::Send {
                text: format!("echo running-$((6*7)); {command}"),
                enter: true,
                ..Default::default()
            }),
        );
        self.expect_on_screen(
            "running-42",
            &format!("the shell never ran {command:?}, so nothing would echo what was typed"),
        );
    }

    /// Waits for the pane's screen to show something, or says what it showed instead.
    pub(crate) fn expect_on_screen(&self, needle: &str, impact: &str) {
        until(
            &format!("{needle:?} to reach the pane"),
            || self.bridge.lines().iter().any(|line| line.contains(needle)),
            || self.bridge.diagnosis(impact),
        );
    }

    /// Waits for the core to write a record carrying `needle` to its run log.
    pub(crate) fn expect_logged(&self, needle: &str, impact: &str) {
        let read = || std::fs::read_to_string(&self.log).unwrap_or_default();
        until(
            &format!("the core to log {needle:?}"),
            || read().contains(needle),
            || format!("  Impact: {impact}.\n  The run log is at {}.", self.log.display()),
        );
    }
}

/// One press, as the shell reports it.
///
/// A builder rather than arguments, because a realistic keystroke has several parts and
/// most presses set none of them: what a test is about is the one or two it does set.
#[derive(Default)]
pub(crate) struct Press {
    key: String,
    text: String,
    without_option: String,
    modifiers: Vec<String>,
    consumed: Vec<String>,
}

impl Press {
    /// A physical key plus whatever the layout produced, which is what a US-layout press
    /// with no modifiers actually looks like. Everything else is absent rather than
    /// defaulted.
    pub(crate) fn new(key: &str, text: &str) -> Press {
        Press { key: key.to_string(), text: text.to_string(), ..Press::default() }
    }

    /// Modifiers held, and which of them the layout spent producing the text.
    ///
    /// The second half is the one that matters and the one macOS will not answer: it is
    /// ghostty's heuristic, and it is what decides whether option composed a character or
    /// asked for a meta chord.
    pub(crate) fn modifiers(mut self, held: &[&str], consumed: &[&str]) -> Press {
        self.modifiers = held.iter().map(|m| (*m).to_string()).collect();
        self.consumed = consumed.iter().map(|m| (*m).to_string()).collect();
        self
    }

    /// What the layout would have produced without option, which the shell reports whenever
    /// option is down. A press that sets modifiers but not this is one macOS never makes.
    pub(crate) fn without_option(mut self, text: &str) -> Press {
        self.without_option = text.to_string();
        self
    }

    pub(crate) fn send(self) {
        assert_ok(&answer(request::Payload::KeyDown(KeyDown {
            key: Some(KeyEvent {
                action: "press".to_string(),
                key: self.key,
                text: self.text,
                text_without_option: self.without_option,
                modifiers: self.modifiers,
                consumed_modifiers: self.consumed,
                ..KeyEvent::default()
            }),
            ..KeyDown::default()
        })));
    }
}

pub(crate) fn answer(payload: request::Payload) -> Response {
    let request = Request::new(payload);
    Response::decode(muster::dispatch(&request.encode_to_vec()).as_slice())
        .expect("the core answers every request with a decodable response")
}

pub(crate) fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => panic!("the core refused: {}", failure.reason),
        None => panic!("the core answered with no payload"),
        // Anything else is the core accepting: what it answers with is the request's business
        // and not this helper's.
        Some(_) => {}
    }
}

/// Attaches, and names the cause that is hardest to see from the refusal.
///
/// A config file Muster refuses is not a failure it stops for: it falls back to finding a
/// daemon on this machine, which in a test run is not the scratch one. The refusal then reads
/// as "no pane called p1..." alongside a pane count from a session nobody here created, and
/// the actual mistake - a typo in the config this test wrote - is nowhere in it.
fn attach_or_explain(pane: &str, config: &Path) -> Attached {
    let response = answer(request::Payload::AttachPane(AttachPane { pane_id: pane.to_string() }));
    match response.payload {
        Some(response::Payload::Attached(attached)) => attached,
        Some(response::Payload::Failure(failure)) => panic!(
            "the core would not attach {pane}: {}\n  Impact: this test has no pane to type \
             into.\n  Most likely: the config at {} was refused, so the core fell back to \
             whatever daemon this machine has rather than the scratch one - note whether the \
             pane count above matches a session you recognise. In TOML a bare key written \
             after `[[daemon]]` belongs to that block, so settings go above it.",
            failure.reason,
            config.display(),
        ),
        other => panic!("expected an attachment, got {other:?}"),
    }
}

/// Re-exported so the tests beside this one keep saying `support::until`, which is where they
/// reach for everything else they share.
pub(crate) use muster_harness::until;

/// A real bridge process, and everything it said.
///
/// Killed on drop, including on a panic: a leaked bridge holds a stream open against a daemon
/// that is about to be torn down under it.
pub(crate) struct Bridge {
    process: Child,
    frames: Arc<Mutex<Vec<u8>>>,
    complaints: Arc<Mutex<String>>,
}

impl Bridge {
    /// Starts a bridge for `pane` the way a surface does, reporting to the window on
    /// `link_socket`.
    pub(crate) fn spawn(pane: &str, daemon_socket: &Path, link_socket: &str) -> Bridge {
        let mut process = Command::new(env!("CARGO_BIN_EXE_muster-bridge"))
            .arg(pane)
            .arg("--daemon-socket")
            .arg(daemon_socket)
            .args(["--app-socket", link_socket])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("cargo builds muster-bridge before this test runs");

        let frames = drain(process.stdout.take().expect("stdout was piped"));
        let complaints = Arc::new(Mutex::new(String::new()));
        std::thread::spawn({
            let stderr = process.stderr.take().expect("stderr was piped");
            let complaints = Arc::clone(&complaints);
            move || {
                let mut reader = stderr;
                let mut chunk = [0u8; 4096];
                while let Ok(read) = reader.read(&mut chunk)
                    && read > 0
                {
                    complaints
                        .lock()
                        .expect("a panicking reader poisoned the bridge's complaints")
                        .push_str(&String::from_utf8_lossy(&chunk[..read]));
                }
            }
        });

        Bridge { process, frames, complaints }
    }

    /// What a surface would be showing, computed by the engine that would show it.
    ///
    /// Replayed from the start of the stream on every call rather than kept live, because a
    /// pane's stream opens with a full repaint and replaying it is exact. It is also a few
    /// kilobytes, so the cost is not worth a terminal held across threads.
    pub(crate) fn grid(&self) -> Grid {
        let mut terminal = Terminal::new(COLUMNS, ROWS).expect("libghostty-vt gives us a terminal");
        terminal.write(&self.frames.lock().expect("a panicking reader poisoned the frame buffer"));
        terminal.viewport(COLUMNS, ROWS)
    }

    /// The screen as text, one string per row, trailing blanks cut.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.grid().rows.iter().map(|row| row.text().trim_end().to_string()).collect()
    }

    /// Whether this bridge has stopped on its own.
    pub(crate) fn has_exited(&mut self) -> bool {
        self.process.try_wait().is_ok_and(|exited| exited.is_some())
    }

    /// Ends this bridge the way a machine going away would, and waits for it to be gone.
    ///
    /// A signal rather than a clean exit, deliberately: what is being proven is that the app
    /// notices a bridge that is simply not there any more, and a process that was shot has no
    /// chance to say anything on its way out.
    pub(crate) fn kill(&mut self) {
        self.process.kill().expect("the bridge is still running");
        self.process.wait().expect("and it can be reaped");
    }

    pub(crate) fn diagnosis(&self, impact: &str) -> String {
        let complaints =
            self.complaints.lock().expect("a panicking reader poisoned the complaints").clone();
        format!(
            "  Impact: {impact}.\n  The pane's screen:\n{}\n  The bridge said:\n{}",
            self.grid().render(),
            if complaints.is_empty() { "    (nothing)".to_string() } else { complaints }
        )
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// Accumulates a stream on a thread, so a poll can read it without blocking on it.
fn drain(mut source: impl Read + Send + 'static) -> Arc<Mutex<Vec<u8>>> {
    let collected = Arc::new(Mutex::new(Vec::new()));
    let writing = Arc::clone(&collected);
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(read) = source.read(&mut chunk)
            && read > 0
        {
            writing.lock().expect("a panicking reader poisoned the buffer").extend(&chunk[..read]);
        }
    });
    collected
}
