//! Handing a pane back at the right size, against a real daemon.
//!
//! A controlling client drives a pane's real PTY and herdr does not release that on detach:
//! measured, a pane went 53x23 to 120x40 and stayed at 120x40 ten seconds after the controller
//! left (`observations/herdr-0.8.0.md` section 4). So a Muster that quits leaves every pane it
//! touched sized to a window that no longer exists, and whoever opens the session next inherits
//! it. This is the check that Muster puts them back.
//!
//! Read through the pane's own shell rather than through the daemon's API, because the daemon
//! does not publish a pane's columns anywhere - `scroll.viewport_rows` is the whole of what it
//! says about geometry. `stty size` is what the probe that recorded section 4 used, and it is
//! the only thing that can tell a pane handed back correctly from one handed back a column
//! narrow. So this fails with both numbers in the message rather than shipping panes that come
//! back slightly wrong.
//!
//! The bridge is stood in for rather than started. In a window the app's message reaches the
//! daemon through `muster-bridge`, which copies whatever the app sends onto the control
//! stream's stdin verbatim and does nothing else with it - and starting a real one here needs a
//! pty, a staged daemon binary beside it and a frame pump, which would make this a test about
//! process plumbing. What the bridge does with the line is `control_socket.rs`'s subject; what
//! this is about is whether the app sends one, and whether the number in it is right.
//!
//! One test in this binary, for the reason the others here are: the seam holds one session per
//! process, and this one quits it.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use herdr_harness::{Daemon, until};
use muster::proto::{
    Event, OpenWindow, Quitting, Request, Response, Startup, ViewChanged, ViewNode, event, request,
    response, view_node,
};
use prost::Message;
use serde_json::{Value, json};

/// What a window takes a pane to. Nothing like the size any daemon lays a pane out at, which
/// is the point: a restore that did nothing has to fail here.
const WINDOW: (u16, u16) = (120, 40);

#[test]
fn quitting_hands_every_pane_back_at_the_size_its_daemon_draws_it() {
    // Nothing here paints a pane, so it never becomes typeable - an error, which opens the
    // roster and republishes. Harmless and noisy, so it is switched off rather than waited out.
    // SAFETY: nothing else in this process reads the environment concurrently. This runs
    // before the daemon is started and before any pane opens, which is when the core reads it.
    unsafe { std::env::set_var("MUSTER_TYPEABLE_DEADLINE_MS", "0") };

    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "geometry", "focus": true }));
    let first = only_pane(&daemon);

    muster::ffi::muster_set_event_callback(Some(note_view));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    quitting_puts_a_pane_back(&daemon, std::slice::from_ref(&first));

    // And every pane, each at its own size. Two side by side, because one would not tell a
    // per-pane answer from the tab's own width handed to everything in it.
    daemon.call("pane.split", &json!({ "target_pane_id": first, "direction": "right" }));
    let second = panes(&daemon)
        .into_iter()
        .find(|pane| pane != &first)
        .expect("the split gives this tab a second pane");
    until(
        "the window to open onto both panes, with a channel for each",
        || sockets().len() == 2,
        || format!("the last view the core published: {:?}", latest_view()),
    );
    quitting_puts_a_pane_back(&daemon, &[first, second]);
}

/// Takes some panes to a window's size, quits, and checks where they landed.
///
/// Against the daemon's own layout rather than against what the pane was before, because what
/// it was before is not a stable answer to anything: herdr starts a pane's terminal at 80x24
/// and settles it into its layout later, and it does not resize an unattached pane at all when
/// its tab is rearranged. So a pane handed back is very often a size it was never at, which is
/// the intended behaviour - the size the next client will draw is the layout's, and the size it
/// had is not recoverable by any client.
///
/// The column the daemon keeps for itself is the one number stated twice - here and in
/// `PaneCells::BORDER_COLUMNS` - and deliberately: a test that derived it the same way the code
/// does would pass whatever the code did. It is measured rather than reasoned about, in
/// `corpus/herdr-0.8.0/geometry/FACTS.json`, where a pane laid out at 54x23 runs a PTY of
/// 53x23 with nothing attached.
fn quitting_puts_a_pane_back(daemon: &Daemon, panes: &[String]) {
    let mut controllers: Vec<Child> = panes.iter().map(|pane| take(daemon, pane)).collect();
    let wanted: Vec<(u16, u16)> = panes.iter().map(|pane| laid_out(daemon, pane)).collect();

    assert_ok(&answer(request::Payload::Quitting(Quitting {})));

    // Read before the controllers are killed, so nothing here can be explained by them going.
    let after: Vec<(u16, u16)> = panes.iter().map(|pane| size(daemon, pane)).collect();
    for controller in &mut controllers {
        let _ = controller.kill();
    }

    for ((pane, held), wanted) in panes.iter().zip(&after).zip(&wanted) {
        assert_ne!(
            *held, WINDOW,
            "quitting left {pane} at the window's size, so it was not handed back at all - and \
             a terminal opened on this session now renders into a grid the wrong shape, which \
             is the whole thing this restore exists to prevent"
        );
        assert_eq!(
            held, wanted,
            "quitting left {pane} at {held:?} and the daemon lays it out at {wanted:?}. If the \
             daemon has changed how much of a pane's rectangle it keeps for itself, the number \
             to move is PaneCells::BORDER_COLUMNS in crates/muster-herdr/src/layout.rs - and \
             this line, which states it a second time on purpose."
        );
    }
}

/// How big the daemon's own layout makes this pane's grid.
///
/// Its rectangle less the column herdr keeps inside it. The rectangle is in cells of a terminal
/// area herdr holds for itself, fixed whether a client is attached or not - which is exactly
/// what makes it the size worth handing a pane back at.
fn laid_out(daemon: &Daemon, pane: &str) -> (u16, u16) {
    let answer = daemon.call("pane.layout", &json!({ "pane_id": pane }));
    let rect = answer["layout"]["panes"]
        .as_array()
        .unwrap_or_else(|| panic!("a pane layout lists its panes: {answer}"))
        .iter()
        .find(|held| held["pane_id"].as_str() == Some(pane))
        .unwrap_or_else(|| panic!("the layout for {pane} does not mention it: {answer}"))["rect"]
        .clone();
    let number = |key: &str| {
        u16::try_from(rect[key].as_i64().unwrap_or_else(|| panic!("a rect has {key}: {rect}")))
            .unwrap_or_else(|_| panic!("a rect's {key} fits a terminal: {rect}"))
    };
    (number("width") - 1, number("height"))
}

/// Takes a pane the way the app's bridge does, and relays the app's answer back.
///
/// One control stream at the window's size, which is what puts the pane's PTY where herdr will
/// not let go of it, plus the copy a bridge makes of whatever the app sends. Both are needed to
/// stage the problem at all: without the stream nothing has taken the pane hostage, and without
/// the copy the app's message reaches a socket with nobody on the other end.
fn take(daemon: &Daemon, pane: &str) -> Child {
    let socket = sockets()
        .get(pane)
        .cloned()
        .unwrap_or_else(|| panic!("the window opened no channel for {pane}: {:?}", sockets()));
    let mut controller = control_stream(daemon, pane, WINDOW);
    relay(&socket, controller.stdin.take().expect("piped"));
    until(
        "the pane to be held at the window's size",
        || size(daemon, pane) == WINDOW,
        || format!("{pane} is {:?}, and should be {WINDOW:?}", size(daemon, pane)),
    );
    controller
}

/// Copies whatever the core writes to a pane's socket onto the control stream's stdin.
///
/// Byte for byte and line by line, which is exactly what `muster-bridge` does with the same
/// bytes: the app writes the daemon's own control-stream JSON, so a bridge is a copy and
/// nothing here is a second implementation of anything. Lines are reassembled rather than
/// forwarded as they arrive, because herdr parses its stdin as newline-delimited JSON and half
/// a message desynchronizes everything after it.
///
/// On a thread, because the core waits for the daemon to agree before it answers a quit - so
/// nothing may relay the message afterwards.
fn relay(socket_path: &str, mut stdin: ChildStdin) {
    let socket = UnixStream::connect(socket_path).unwrap_or_else(|e| {
        panic!("the core is listening on {socket_path} and it would not take a connection: {e}")
    });
    std::thread::spawn(move || {
        let mut reader = BufReader::new(socket);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if stdin.write_all(&line).is_err() {
                return;
            }
            let _ = stdin.flush();
        }
    });
}

/// A controlling client on this pane, at a given size.
///
/// The same command the bridge runs, for the same reason: this is what takes a pane's geometry
/// hostage, so staging the problem means doing what the app does. Its stdout is drained on a
/// thread - a control stream publishes frames continuously, and a full pipe stops the daemon
/// answering anything at all.
fn control_stream(daemon: &Daemon, pane: &str, (columns, rows): (u16, u16)) -> Child {
    let mut child = Command::new(herdr_harness::binary())
        .env("HERDR_SOCKET_PATH", daemon.socket_path())
        .args(["terminal", "session", "control", pane])
        .args(["--cols", &columns.to_string(), "--rows", &rows.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("could not open a control stream on {pane}: {e}"));

    let mut frames = child.stdout.take().expect("piped");
    std::thread::spawn(move || std::io::copy(&mut frames, &mut std::io::sink()));
    child
}

/// What a pane's own shell says its terminal is, as columns and rows.
///
/// The daemon publishes a pane's rows and never its columns, so the shell is the only oracle
/// that can answer this - which is also why the restore cannot simply read a size back and put
/// it there.
fn size(daemon: &Daemon, pane: &str) -> (u16, u16) {
    // A marker nothing on this screen can already be carrying. The pane keeps every answer this
    // function has ever asked for, and a shared marker would let a stale line satisfy a fresh
    // question - which is a size read from before the resize under test, arriving instantly and
    // looking exactly like a resize that did not happen.
    let asked = ASKED.fetch_add(1, Ordering::Relaxed);
    let marker = format!("mstr{asked}-");
    daemon.call(
        "pane.send_text",
        &json!({ "pane_id": pane, "text": format!("echo {marker}$(stty size | tr ' ' x)\n") }),
    );
    let mut found = None;
    until(
        "the pane's shell to report its size",
        || {
            found = read(daemon, pane)
                .lines()
                .rev()
                // The command line echoes the marker too, and that is the line still carrying
                // `stty` - so the answer is the last one that does not.
                .find(|line| line.contains(&marker) && !line.contains("stty"))
                .and_then(|line| line.rsplit(&marker).next().map(str::to_string))
                .and_then(|answer| {
                    let (rows, columns) = answer.trim().split_once('x')?;
                    Some((columns.trim().parse().ok()?, rows.trim().parse().ok()?))
                });
            found.is_some()
        },
        || format!("the pane reads:\n{}", read(daemon, pane)),
    );
    found.expect("the wait above returned because there was an answer")
}

/// How many sizes have been asked for, so each question is its own.
static ASKED: AtomicUsize = AtomicUsize::new(0);

/// Where each pane on screen wants its bridge to dial back, by the name its *daemon* knows it
/// by - which is what everything else here is holding.
///
/// From the view rather than from an attach reply, because this window opened onto whatever the
/// daemon already held rather than being pointed at a pane - which is what `muster` with no
/// arguments does, and the case a person quitting is almost always in. The view carries both
/// spellings of a pane, which is the only reason this correlation is possible at all.
fn sockets() -> BTreeMap<String, String> {
    let Some(view) = latest_view() else { return BTreeMap::new() };
    view.regions
        .iter()
        .filter_map(|region| region.root.as_ref())
        .flat_map(leaves)
        .filter(|(_, socket)| !socket.is_empty())
        .collect()
}

fn leaves(node: &ViewNode) -> Vec<(String, String)> {
    match &node.node {
        Some(view_node::Node::Pane(pane)) => {
            vec![(pane.backend_pane_id.clone(), pane.control_socket_path.clone())]
        }
        Some(view_node::Node::Split(split)) => {
            split.first.iter().chain(split.second.iter()).flat_map(|child| leaves(child)).collect()
        }
        None => Vec::new(),
    }
}

fn read(daemon: &Daemon, pane: &str) -> String {
    let read = daemon.call("pane.read", &json!({ "pane_id": pane, "source": "recent_unwrapped" }));
    read.get("read")
        .and_then(|read| read.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a pane read carries its text under `read`: {read}"))
        .to_string()
}

fn panes(daemon: &Daemon) -> Vec<String> {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    snapshot["snapshot"]["panes"]
        .as_array()
        .unwrap_or_else(|| panic!("no panes in {snapshot}"))
        .iter()
        .filter_map(|pane| pane["pane_id"].as_str())
        .map(str::to_string)
        .collect()
}

fn only_pane(daemon: &Daemon) -> String {
    let panes = panes(daemon);
    assert_eq!(panes.len(), 1, "a fresh workspace should hold exactly one pane: {panes:?}");
    panes[0].clone()
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);

extern "C" fn note_view(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::ViewChanged(view)) = event.payload {
        *VIEW.lock().expect("a panicking test poisoned the view") = Some(view);
    }
}

fn latest_view() -> Option<ViewChanged> {
    VIEW.lock().expect("a panicking test poisoned the view").clone()
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Made(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
