//! What typing into Muster actually sets in motion, with nothing faked.
//!
//! Every piece under this is already judged on its own: the keymap and the encoder by the
//! conformance corpus, the frame decoder by recorded bytes, the daemon connection by
//! `muster-herdr`'s suite. What none of them can say is whether the pieces are joined. Here
//! a keystroke enters at the seam, is encoded, crosses a socket into a real `muster-bridge`,
//! reaches a real herdr, runs a real program, and comes back as frames.
//!
//! Both routes out of the core are exercised, because they fail differently. Printable keys
//! are encoded locally and leave over the bridge's socket; arrows are handed to the daemon
//! to encode against the pane's real modes and never touch the bridge (`architecture.md`,
//! control plane). `cat -v` runs in the pane so that what arrived is legible on the screen
//! rather than inferred: an escape sequence renders as `^[[A`.
//!
//! One test in this binary, on purpose. The seam holds the attached pane in a process
//! global, and this points the whole process at a scratch daemon through the environment.
//! A second test here would race both.

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
const COLUMNS: u16 = 80;
const ROWS: u16 = 24;

#[test]
fn a_keystroke_crosses_the_seam_and_arrives_on_the_panes_screen() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "input", "focus": true }));
    let pane = only_pane(&daemon);

    // The core discovers its daemon the way a person's would, from the environment, so this
    // is the only way to point it at a scratch one.
    //
    // SAFETY: this binary holds one test, so the only other thread alive is the harness's
    // own, which reads no environment. The module docs say why it stays that way.
    unsafe { std::env::set_var("HERDR_SOCKET_PATH", daemon.socket_path()) };

    assert_ok(&answer(request::Payload::Startup(Startup::default())));

    // Registered before the attach that binds the socket, because the bridge can dial back
    // before this test's next line runs.
    muster::ffi::muster_set_event_callback(Some(note_typeable));
    let attached = attach(&pane);

    let bridge = Bridge::spawn(&pane, &attached.control_socket_path, &daemon);
    until(
        "the bridge to dial the core back",
        || TYPEABLE.load(Ordering::Relaxed),
        || bridge.diagnosis("nothing became typeable"),
    );

    // Setup goes through herdr's own API rather than through the path under test, so that a
    // broken input path fails at the assertion rather than at the arrangement.
    //
    // Application cursor keys go on first, and that is what makes the arrow below worth
    // asserting. In a pane's default mode both routes encode Up as ESC [ A, so a test there
    // passes whether or not the daemon did the encoding. Under DECCKM the correct answer is
    // ESC O A and Muster's blind profile still says ESC [ A
    // (`muster_core::input::TerminalModeProfile::UNKNOWN_PANE`), so the two are finally
    // distinguishable - which is the whole reason this key leaves the core over a different
    // channel.
    daemon.call(
        "pane.send_input",
        &json!({ "pane_id": pane, "text": "printf '\\033[?1h'; cat -v\n" }),
    );
    until(
        "cat to take the pane",
        || foreground_processes(&daemon, &pane).iter().any(|name| name == "cat"),
        || bridge.diagnosis("cat never started, so nothing would have echoed what was typed"),
    );

    // The local route. `muster` appears twice: once echoed by the line discipline, which
    // says the bytes reached the PTY, and once written by cat, which says the program read
    // them.
    for key in ["KeyM", "KeyU", "KeyS", "KeyT", "KeyE", "KeyR"] {
        assert_ok(&answer(press(key, &key.trim_start_matches("Key").to_lowercase())));
    }
    assert_ok(&answer(press("Enter", "")));
    until(
        "the typed line to come back from cat",
        || bridge.lines().iter().filter(|line| *line == "muster").count() >= 2,
        || bridge.diagnosis("the line never arrived, or arrived only as the terminal's echo"),
    );

    // The server-encoded route, which leaves the core for the daemon directly and skips the
    // bridge. `^[OA` is cat -v's rendering of ESC O A, the sequence herdr chose by reading
    // modes Muster cannot see. `^[[A` here would mean the arrow fell back to the local
    // guess, which is the regression this exists to catch.
    assert_ok(&answer(press("ArrowUp", "")));
    until(
        "the arrow herdr encoded to reach the pane",
        || bridge.lines().iter().any(|line| line.contains("^[OA")),
        || {
            bridge.diagnosis(
                "the arrow reached nothing, or reached the pane as the locally guessed \
                 ESC [ A rather than the ESC O A this pane's modes call for",
            )
        },
    );
}

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

fn answer(payload: request::Payload) -> Response {
    let request = Request { payload: Some(payload) };
    Response::decode(muster::dispatch(&request.encode_to_vec()).as_slice())
        .expect("the core answers every request with a decodable response")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Attached(_)) => {}
        Some(response::Payload::Failure(failure)) => panic!("the core refused: {}", failure.reason),
        None => panic!("the core answered with no payload"),
    }
}

fn attach(pane: &str) -> Attached {
    let response = answer(request::Payload::AttachPane(AttachPane { pane_id: pane.to_string() }));
    match response.payload {
        Some(response::Payload::Attached(attached)) => attached,
        other => panic!("expected an attachment, got {other:?}"),
    }
}

/// One press, as the shell reports it: a physical key name plus whatever the layout
/// produced. Everything else about a keystroke is absent rather than defaulted, which is
/// what a US-layout press with no modifiers actually looks like.
fn press(key: &str, text: &str) -> request::Payload {
    request::Payload::KeyDown(KeyDown {
        key: Some(KeyEvent {
            action: "press".to_string(),
            key: key.to_string(),
            text: text.to_string(),
            ..KeyEvent::default()
        }),
        ..KeyDown::default()
    })
}

fn only_pane(daemon: &Daemon) -> String {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    let panes = snapshot
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("panes"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no panes in {snapshot}"));
    assert_eq!(panes.len(), 1, "a fresh workspace should hold exactly one pane: {panes:?}");
    panes[0].get("pane_id").and_then(Value::as_str).expect("a pane carries an id").to_string()
}

fn foreground_processes(daemon: &Daemon, pane: &str) -> Vec<String> {
    let info = daemon.call("pane.process_info", &json!({ "pane_id": pane }));
    info.get("process_info")
        .and_then(|info| info.get("foreground_processes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|process| process.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Polls a condition, and says what it was waiting for and what the pane looked like.
///
/// Polling rather than sleeping: herdr answers in under a millisecond, so a sleep long
/// enough to be safe makes the suite unpleasant and one short enough to be pleasant is
/// flaky on a loaded machine. The third argument is what turns a timeout from "something
/// did not happen" into a screen someone can read.
fn until(what: &str, mut ready: impl FnMut() -> bool, on_failure: impl FnOnce() -> String) {
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
struct Bridge {
    process: Child,
    frames: Arc<Mutex<Vec<u8>>>,
    complaints: Arc<Mutex<String>>,
}

impl Bridge {
    fn spawn(pane: &str, control_socket: &str, daemon: &Daemon) -> Bridge {
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
    fn grid(&self) -> Grid {
        let terminal = Terminal::new(COLUMNS, ROWS).expect("libghostty-vt gives us a terminal");
        terminal.write(&self.frames.lock().expect("a panicking reader poisoned the frame buffer"));
        terminal.viewport(COLUMNS, ROWS)
    }

    /// The screen as text, one string per row, trailing blanks cut.
    fn lines(&self) -> Vec<String> {
        self.grid().rows.iter().map(|row| row.text().trim_end().to_string()).collect()
    }

    fn diagnosis(&self, impact: &str) -> String {
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
