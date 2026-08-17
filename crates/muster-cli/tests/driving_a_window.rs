//! Whether the command somebody actually types drives a real window.
//!
//! The real binary, spawned as a process, against a real endpoint and a real daemon. Everything in
//! between has no other test: `argv.rs` proves a command line becomes the right request and
//! `muster-seam/tests/command.rs` proves the endpoint answers one, and neither of them would notice
//! that the binary cannot find the socket, renders nothing, or exits zero on a refusal. Each of
//! those failures looks the same from a pane: the CLI does nothing.
//!
//! One test in this binary, on purpose: the seam holds one session per process.
//!
//! The child's environment is cleared rather than inherited, and that is load-bearing rather than
//! tidy. This suite is developed inside Muster, so an inherited `MUSTER_SOCKET` would point the
//! test at the developer's own window - splitting real panes and typing into them.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use herdr_harness::Daemon;
use muster::proto::{OpenWindow, Request, Response, Startup, request, response};
use prost::Message;
use serde_json::{Value, json};

#[test]
fn a_pane_can_drive_the_window_it_is_drawn_in() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "driven", "focus": true }));

    // Inside the daemon's own scratch directory, so the run leaves nothing behind and two runs of
    // this test in parallel cannot collide on one path.
    let socket = daemon.root().join("command.sock");
    let socket = socket.to_string_lossy().into_owned();
    accepted(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.clone(),
        ..Startup::default()
    })));
    accepted(&dispatch(request::Payload::OpenWindow(OpenWindow {})));

    // What a pane Muster made is handed. From here on this test is exactly what a program inside
    // that pane can do, and nothing else.
    let inside =
        |pane: &str| vec![("MUSTER_SOCKET", socket.clone()), ("MUSTER_PANE", pane.to_string())];

    let first = until("the window to describe the pane the daemon holds", || {
        let window = json_from(&run(&["window", "--json"], &inside("")));
        let pane = window["panes"].get(0)?.clone();
        Some(pane["pane"].as_str()?.to_string())
    });
    assert!(
        first.starts_with('p'),
        "the CLI is answered with Muster's own name for a pane, never the daemon's - a herdr id is \
         not unique across machines and is not addressable. Got {first:?}"
    );

    // The gesture the cards are about: an agent in a pane makes another pane below itself, names
    // it, and is told what it is called - naming no pane, because it is standing in one.
    let made = run(&["pane", "new", "--down", "--name", "🤖 A"], &inside(&first));
    assert_eq!(made.code, 0, "`muster pane new` failed: {}", made.errors);
    let made_pane = made.out.trim().to_string();
    assert!(
        made_pane.starts_with('p') && made_pane != first,
        "`muster pane new` printed {made_pane:?}, which is not the name of a new pane. A caller \
         that cannot learn the name cannot address what it just made, and the name was minted \
         inside that call."
    );

    let window = until("the window to list the pane under the name the split asked for", || {
        let window = json_from(&run(&["window", "--json"], &inside(&first)));
        let named = window["panes"]
            .as_array()?
            .iter()
            .any(|pane| pane["pane"] == json!(made_pane) && pane["given_name"] == json!("🤖 A"));
        named.then_some(window)
    });

    // The keyboard stayed where it was, asked of the CLI's own answer rather than of the core:
    // what a script means by making a pane is `leave my cursor alone`, and the flag that says
    // otherwise defaults to off.
    assert_eq!(
        window["keyboard"],
        json!(first),
        "a split that did not ask for focus took it anyway: {window}"
    );

    // The other rendering of the same answer. Read here rather than eyeballed because it is what a
    // person sees, and because a pane whose name lines up in a column is the whole reason the
    // column widths are computed at all.
    let readable = run(&["window"], &inside(&first));
    assert_eq!(readable.code, 0, "`muster window` failed: {}", readable.errors);
    for expected in [first.as_str(), made_pane.as_str(), "🤖 A", "local", "tab 1"] {
        assert!(
            readable.out.contains(expected),
            "`muster window` said nothing about {expected:?}, so somebody reading it cannot see \
             what the window holds:\n{}",
            readable.out
        );
    }
    // The keyboard, marked in the gutter of its own row rather than anywhere else on the page - so
    // this is asserted on the line and not on the output, which is the whole difference between
    // saying which pane has it and merely mentioning that some pane does.
    let has_keyboard: Vec<&str> =
        readable.out.lines().filter(|line| line.trim_start().starts_with('▸')).collect();
    assert_eq!(
        has_keyboard.len(),
        1,
        "exactly one pane has the window's keyboard, and {} rows are marked as having it:\n{}",
        has_keyboard.len(),
        readable.out
    );
    assert!(
        has_keyboard[0].contains(&first),
        "the keyboard is marked on the wrong row - it is on {first}, and the marked row is \
         {:?}",
        has_keyboard[0]
    );
    assert!(
        !readable.out.contains('\u{1b}'),
        "`muster window` wrote colour escapes into a pipe. Anything reading this output has to \
         strip them, and a pane name with one in the middle is not the name:\n{:?}",
        readable.out
    );

    // Text to a pane by name, which is an agent instructing another. Read off the filesystem
    // rather than the pane's screen: a grid wraps at its width and carries the shell's own echo of
    // the command, so reading one cannot tell `it ran` from `it is sitting at the prompt`.
    let told = daemon.root().join("told.txt");
    let sending = format!("printf 'told' > {}", told.display());
    let sent = run(&["pane", "send", "--pane", &made_pane, &sending, "--enter"], &inside(&first));
    assert_eq!(sent.code, 0, "`muster pane send` failed: {}", sent.errors);
    until_file(&told, "text sent to a pane by name to have run there");

    a_mistyped_flag_is_refused_and_says_what_was_meant(&inside(&first));
    with_no_window_to_ask_nothing_is_guessed(daemon.root());
}

/// A refusal an agent can act on, and an exit code a script can branch on.
///
/// Worth its own check because the failure is quiet: a CLI that exits zero having done nothing
/// makes a broken script look like a window that ignored it.
fn a_mistyped_flag_is_refused_and_says_what_was_meant(environment: &[(&str, String)]) {
    let refused = run(&["pane", "new", "--focused"], environment);
    assert_eq!(
        refused.code, 2,
        "a command line that could not be read should exit 2, and exited {}. stderr:\n{}",
        refused.code, refused.errors
    );
    assert!(
        refused.errors.contains("--focus"),
        "the refusal for `--focused` does not mention `--focus`, so whoever typed it has to go \
         and read the help:\n{}",
        refused.errors
    );
    assert!(
        refused.out.is_empty(),
        "a refusal was written to stdout, where a script reading an answer would find it: {:?}",
        refused.out
    );
}

/// No window, and nothing guessed about it.
///
/// The state every caller outside a pane starts in, and the one where a wrong answer is worst: a
/// CLI that picked whichever socket it found first would drive a window nobody meant.
fn with_no_window_to_ask_nothing_is_guessed(root: &Path) {
    let empty = root.join("no-muster-here");
    std::fs::create_dir_all(empty.join("state")).expect("a scratch directory can be made");
    let asked = run(&["window"], &[("MUSTER_HOME", empty.to_string_lossy().into_owned())]);
    assert_eq!(
        asked.code, 3,
        "with no window to ask this should exit 3, distinct from a refusal - a script retries one \
         and not the other. Got {} with stderr:\n{}",
        asked.code, asked.errors
    );
    assert!(
        asked.errors.contains("--socket"),
        "the refusal does not say what to do about it, and there is something to do:\n{}",
        asked.errors
    );
}

struct Ran {
    code: i32,
    out: String,
    errors: String,
}

/// The real binary, with an environment that says exactly what it is being given.
fn run(argv: &[&str], environment: &[(&str, String)]) -> Ran {
    let mut command = Command::new(env!("CARGO_BIN_EXE_muster"));
    command.args(argv).env_clear();
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("the muster binary could not be run: {error}"));
    Ran {
        code: output.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&output.stdout).into_owned(),
        errors: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn json_from(ran: &Ran) -> Value {
    assert_eq!(ran.code, 0, "`muster window --json` failed: {}", ran.errors);
    serde_json::from_str(&ran.out).unwrap_or_else(|error| {
        panic!("`muster --json` wrote something that is not JSON ({error}): {:?}", ran.out)
    })
}

/// Dispatches straight into the core, for the two things only the app can do.
///
/// Startup and open are the shell's job, so they arrive over the C ABI rather than the socket -
/// there is no endpoint to dial until the first of them has been answered.
fn dispatch(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn accepted(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}

/// Polls rather than sleeping, and says what it was waiting for.
fn until<T>(what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(value) = ready() {
            return value;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("waited 20s for {what}, and it never happened");
}

/// Waits for a file a shell in a pane was asked to write.
fn until_file(path: &Path, what: &str) {
    until(what, || std::fs::read_to_string(path).ok().filter(|text| !text.is_empty()));
}
