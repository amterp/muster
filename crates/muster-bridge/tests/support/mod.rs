//! A whole Muster, assembled from real parts, so a test can type into it.
//!
//! Every binary here does the same expensive setup: a scratch daemon, a config pointing the
//! core at it, an attached pane, a real bridge, and a way to read what a surface would be
//! showing. What differs between them is the keystrokes and the config, which is the part
//! worth reading in the test itself.
//!
//! One test per binary, and that is a constraint rather than a style. The seam holds the
//! attached pane in a process global and a `Startup` points the whole process at one config,
//! so two tests in one binary would race both.
//!
//! Each binary uses one slice of this, so whatever it does not touch is dead to it. That is
//! how Rust builds integration tests, not a sign that something here has no readers.
#![allow(dead_code)]

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use herdr_harness::Daemon;
use muster::proto::{
    AttachPane, Attached, Event, KeyDown, KeyEvent, Request, Response, Startup, event, request,
    response,
};
use muster_vt::{Grid, Terminal};
use prost::Message;
use serde_json::{Value, json};

/// The bridge falls back to this when its stdout is a pipe rather than a surface's PTY, so
/// the oracle reads the pane at the size the daemon is rendering it for.
pub(crate) const COLUMNS: u16 = 80;
pub(crate) const ROWS: u16 = 24;

/// Set from the callback the shell would register, which is also the only test of the push
/// direction: the pane becoming typeable is announced, not answered.
static TYPEABLE: AtomicBool = AtomicBool::new(false);

extern "C" fn note_typeable(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    if let Ok(Event { payload: Some(event::Payload::PaneTypeable(_)) }) = Event::decode(bytes) {
        TYPEABLE.store(true, Ordering::Relaxed);
    }
}

/// Everything between a fresh daemon and a pane that can be typed into.
///
/// `config` is whatever the test wants in the file beyond the `[[daemon]]` block naming this
/// daemon - which is how a config-driven behavior gets exercised through the path a person
/// uses, rather than by reaching past the file into the core.
pub(crate) struct Typing {
    pub(crate) daemon: Daemon,
    pub(crate) pane: String,
    pub(crate) bridge: Bridge,
}

impl Typing {
    pub(crate) fn start(config: &str) -> Typing {
        let daemon = Daemon::start();
        daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "input", "focus": true }));
        let pane = only_pane(&daemon);

        // A config file naming this daemon's socket, which is how a person points Muster at
        // a daemon it did not start - and the only way there is, since Muster runs its own
        // herdr and does not read HERDR_SOCKET_PATH.
        let path = daemon.muster_config_with(config);
        assert_ok(&answer(request::Payload::Startup(Startup {
            config_path: path.to_string_lossy().into_owned(),
            ..Startup::default()
        })));

        // Registered before the attach that binds the socket, because the bridge can dial
        // back before the next line runs.
        muster::ffi::muster_set_event_callback(Some(note_typeable));
        let attached = attach_or_explain(&pane, &path);

        let bridge = Bridge::spawn(&pane, &attached.control_socket_path, &daemon);
        until(
            "the bridge to dial the core back",
            || TYPEABLE.load(Ordering::Relaxed),
            || bridge.diagnosis("nothing became typeable"),
        );
        Typing { daemon, pane, bridge }
    }

    /// Runs a program in the pane and waits for it to take over.
    ///
    /// Through herdr's own API rather than through the path under test, so that a broken
    /// input path fails at the assertion rather than at the arrangement.
    pub(crate) fn run(&self, command: &str, process: &str) {
        self.daemon.call(
            "pane.send_input",
            &json!({ "pane_id": self.pane, "text": format!("{command}\n") }),
        );
        until(
            &format!("{process} to take the pane"),
            || self.foreground_processes().iter().any(|name| name == process),
            || {
                self.bridge.diagnosis(&format!(
                    "{process} never started, so nothing would have echoed what was typed"
                ))
            },
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

    pub(crate) fn foreground_processes(&self) -> Vec<String> {
        let info = self.daemon.call("pane.process_info", &json!({ "pane_id": self.pane.as_str() }));
        info.get("process_info")
            .and_then(|info| info.get("foreground_processes"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|process| process.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
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
    let request = Request { payload: Some(payload) };
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

pub(crate) fn attach(pane: &str) -> Attached {
    let response = answer(request::Payload::AttachPane(AttachPane { pane_id: pane.to_string() }));
    match response.payload {
        Some(response::Payload::Attached(attached)) => attached,
        other => panic!("expected an attachment, got {other:?}"),
    }
}

/// Attaches, and names the cause that is hardest to see from the refusal.
///
/// A config file Muster refuses is not a failure it stops for: it falls back to finding a
/// daemon on this machine, which in a test run means the developer's own herdr, with its own
/// panes. The refusal then reads as "no daemon holds a pane called w1:p1" alongside a pane
/// count from a session nobody here created, and the actual mistake - a typo in the config
/// this test wrote - is nowhere in it.
fn attach_or_explain(pane: &str, config: &std::path::Path) -> Attached {
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

pub(crate) fn only_pane(daemon: &Daemon) -> String {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    let panes = snapshot
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("panes"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no panes in {snapshot}"));
    assert_eq!(panes.len(), 1, "a fresh workspace should hold exactly one pane: {panes:?}");
    panes[0].get("pane_id").and_then(Value::as_str).expect("a pane carries an id").to_string()
}

/// Polls a condition, and says what it was waiting for and what the pane looked like.
///
/// Polling rather than sleeping: herdr answers in under a millisecond, so a sleep long
/// enough to be safe makes the suite unpleasant and one short enough to be pleasant is
/// flaky on a loaded machine. The third argument is what turns a timeout from "something
/// did not happen" into a screen someone can read.
pub(crate) fn until(
    what: &str,
    mut ready: impl FnMut() -> bool,
    on_failure: impl FnOnce() -> String,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out after 15s waiting for {what}.\n{}", on_failure());
}

/// A real bridge process, and everything it said.
///
/// Killed on drop, including on a panic: a leaked bridge holds a control stream open and
/// the daemon it holds it against is about to be torn down under it.
pub(crate) struct Bridge {
    process: Child,
    frames: Arc<Mutex<Vec<u8>>>,
    complaints: Arc<Mutex<String>>,
}

impl Bridge {
    pub(crate) fn spawn(pane: &str, control_socket: &str, daemon: &Daemon) -> Bridge {
        // The bridge runs `herdr terminal session control` off PATH, which is right in
        // production and would otherwise reach for whichever version the developer happens
        // to have. The pinned one goes first.
        let pinned = herdr_harness::binary();
        let directory = std::path::Path::new(&pinned)
            .parent()
            .unwrap_or_else(|| panic!("the pinned herdr at {pinned} has no directory"));
        let path = format!("{}:{}", directory.display(), std::env::var("PATH").unwrap_or_default());

        let mut process = Command::new(env!("CARGO_BIN_EXE_muster-bridge"))
            .arg(pane)
            .args(["--control-socket", control_socket])
            .env("PATH", path)
            .env("HERDR_SOCKET_PATH", daemon.socket_path())
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
        let terminal = Terminal::new(COLUMNS, ROWS).expect("libghostty-vt gives us a terminal");
        terminal.write(&self.frames.lock().expect("a panicking reader poisoned the frame buffer"));
        terminal.viewport(COLUMNS, ROWS)
    }

    /// The screen as text, one string per row, trailing blanks cut.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.grid().rows.iter().map(|row| row.text().trim_end().to_string()).collect()
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
