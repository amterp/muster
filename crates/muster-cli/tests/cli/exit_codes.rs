//! What a non-zero exit tells a caller about trying again.
//!
//! The exit code is the only part of a failure a script branches on without reading English, so
//! what it means is a contract rather than a detail. The distinction under test is the one that
//! costs something when it is wrong: a window that was never asked can be asked again, and a
//! window that took the request and said nothing back cannot - retrying that repeats whatever
//! it did. One agent received the same instruction six times because these were one code
//! (kan a_2L1AEIpIY).
//!
//! No app and no daemon, for the reason `two_windows.rs` gives: what is being tested is the
//! CLI's own reading of a socket, and the far end only has to behave in a particular way. The
//! framing and the schema these listeners speak are the real ones.

use std::collections::BTreeMap;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use muster_cli::dial;
use muster_proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster_proto::{PaneStateChanged, ReadWindow, Request, Response, request, response};
use prost::Message;

/// Short, because what is under test is which answer a deadline produces rather than the
/// deadline itself.
const BRIEFLY: Duration = Duration::from_millis(200);

/// Longer than any of these tests take, held so a connection stays open rather than closing
/// under the caller and turning a silence into an end-of-file.
const UNTIL_THE_TEST_IS_OVER: Duration = Duration::from_mins(1);

#[test]
fn a_window_that_answers_nothing_is_not_a_window_that_was_never_there() {
    let socket = socket_at("silent");
    let listener = UnixListener::bind(&socket).expect("a scratch socket path is free");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // The request is read and nothing is written back. This is the case the whole file
            // is about: the bytes are on the far side, and it is the answer that goes missing.
            let _ = read_frame(&mut stream, LARGEST_MESSAGE);
            std::thread::sleep(UNTIL_THE_TEST_IS_OVER);
        }
    });

    let trouble = asked(&socket).expect_err("a listener that says nothing produces no response");

    assert_eq!(
        trouble.code(),
        4,
        "a window that took the request and never answered exits {}, which is the code for a \
         window that was never asked. A caller reading it does what a caller does with a failed \
         request and sends it again - and whatever the first one did happens twice.\n{}",
        trouble.code(),
        trouble.detail()
    );
}

#[test]
fn an_answer_this_muster_cannot_read_is_still_an_answer() {
    let socket = socket_at("gibberish");
    let listener = UnixListener::bind(&socket).expect("a scratch socket path is free");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = read_frame(&mut stream, LARGEST_MESSAGE);
            // A varint tag with nothing after it: a well-formed frame carrying something that
            // is not a `Response`, which is what a window built from another schema looks like.
            let _ = write_frame(&mut stream, &[0x08]);
            std::thread::sleep(UNTIL_THE_TEST_IS_OVER);
        }
    });

    let trouble = asked(&socket).expect_err("a truncated varint is not a Response");

    assert_eq!(
        trouble.code(),
        4,
        "a window that answered with something unreadable exits {}. It answered, so it acted, \
         and a caller told the request was refused will send it again.\n{}",
        trouble.code(),
        trouble.detail()
    );
}

#[test]
fn a_window_whose_daemon_never_answered_exits_as_unanswered() {
    // The same unknown one hop further in. The window answered promptly, and what it said is that
    // its daemon took the request and said nothing back - a message delivered on a loaded machine
    // looked exactly like this, and exited 1 as a refusal (kan a_2LOHfLmsL).
    let socket = socket_at("daemon-silent");
    let listener = UnixListener::bind(&socket).expect("a scratch socket path is free");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = read_frame(&mut stream, LARGEST_MESSAGE);
            let answer = Response::unanswered("timed out");
            let _ = write_frame(&mut stream, &answer.encode_to_vec());
        }
    });

    let response = asked(&socket).expect("the window answered");
    let trouble = muster_cli::render::answer(&response, false)
        .expect_err("a request nobody answered is not a success");

    assert_eq!(
        trouble.code(),
        4,
        "a window reporting that its daemon never answered exits {}. The request reached the \
         daemon, so a caller that sends it again may do it twice.\n{}",
        trouble.code(),
        trouble.detail()
    );
}

#[test]
fn a_socket_nobody_is_listening_on_is_a_window_that_was_never_asked() {
    let socket = socket_at("nobody");

    let trouble = asked(&socket).expect_err("nothing is bound to that path");

    assert_eq!(
        trouble.code(),
        3,
        "a window that could not be dialled at all exits {}, and this is the one failure a \
         caller may safely send again.\n{}",
        trouble.code(),
        trouble.detail()
    );
}

/// A request too big for any window to read is refused before it is sent.
///
/// A window reads the length, refuses it and hangs up without a word, and a request with no
/// answer is exit 4 - "whatever was asked for may well have happened" - about a send that
/// certainly did not. `pane send --file` is what makes one reachable.
#[test]
fn a_request_no_window_would_read_is_refused_rather_than_sent() {
    let socket = socket_at("oversized");
    let listener = UnixListener::bind(&socket).expect("a scratch socket path is free");
    let (heard, hearing) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut length = [0u8; 4];
            let _ = heard.send(std::io::Read::read_exact(&mut stream, &mut length).is_ok());
        }
    });

    let brief = socket.with_extension("md");
    std::fs::write(&brief, "a".repeat(LARGEST_MESSAGE as usize + 1)).expect("/tmp is writable");
    let ran = ran(&[
        "--socket",
        &socket.to_string_lossy(),
        "pane",
        "send",
        "--file",
        &brief.to_string_lossy(),
    ]);

    assert_eq!(
        ran.code, 1,
        "a send no window can read exits {} rather than as refused. Anything but 1 tells a caller \
         the request may have landed, and it cannot have.\n{}",
        ran.code, ran.errors
    );
    assert!(
        !hearing.recv_timeout(BRIEFLY).unwrap_or(false),
        "the oversized request was written to the window anyway, so the refusal describes a send \
         that did in fact go out"
    );
}

/// A watch whose window goes away mid-stream is a window that is not there any more.
///
/// Not 4. Watching changes nothing, so there is nothing on the window's side that could have
/// happened twice, and the caller should run it again once a window is back - which is 3. What
/// the watch had already said still reaches the caller first.
#[test]
fn a_watch_whose_window_hangs_up_exits_as_no_window() {
    let socket = socket_at("watch-hangs-up");
    let listener = UnixListener::bind(&socket).expect("a scratch socket path is free");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = read_frame(&mut stream, LARGEST_MESSAGE);
            let said = Response {
                payload: Some(response::Payload::PaneState(PaneStateChanged {
                    daemon_id: "local".to_string(),
                    pane_id: "p1w3r07bsd".to_string(),
                    state: "working".to_string(),
                    since_ms: 1_757_700_000_000,
                })),
            };
            let _ = write_frame(&mut stream, &said.encode_to_vec());
            // Dropped here, which is what the connection of a window that quit looks like.
        }
    });

    let ran = ran(&["--socket", &socket.to_string_lossy(), "window", "--watch"]);

    assert_eq!(
        ran.code, 3,
        "a watch whose window hung up exits {}. It changed nothing, so the caller should be told \
         it may simply run it again.\n{}",
        ran.code, ran.errors
    );
    assert!(
        ran.out.contains("p1w3r07bsd") && ran.out.contains("working"),
        "what the watch said before the window went away never reached the caller: {:?}",
        ran.out
    );
}

/// A wait that runs out says so with a code of its own.
///
/// A script waiting on an agent branches on this: the agent is still going, which is neither a
/// refusal nor a missing window, and waiting again is harmless.
#[test]
fn a_wait_that_runs_out_exits_five() {
    let socket = socket_at("wait-runs-out");
    let listener = UnixListener::bind(&socket).expect("a scratch socket path is free");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = read_frame(&mut stream, LARGEST_MESSAGE);
            std::thread::sleep(UNTIL_THE_TEST_IS_OVER);
        }
    });

    let ran = ran(&[
        "--socket",
        &socket.to_string_lossy(),
        "pane",
        "wait",
        "--pane",
        "p1w3r07bsd",
        "--until",
        "idle",
        "--timeout",
        "1",
    ]);

    assert_eq!(
        ran.code, 5,
        "a wait that ran out exits {}, so a script cannot tell an agent still working from a \
         window that refused or was never there.\n{}",
        ran.code, ran.errors
    );
    assert!(
        ran.errors.contains("p1w3r07bsd") && ran.errors.contains("idle"),
        "a wait that ran out should say what it was waiting on: {}",
        ran.errors
    );
}

struct Ran {
    code: i32,
    out: String,
    errors: String,
}

/// One run of the command in this process, with nothing on stdin and no environment.
fn ran(argv: &[&str]) -> Ran {
    let argv: Vec<String> = argv.iter().map(ToString::to_string).collect();
    let (mut out, mut errors) = (Vec::new(), Vec::new());
    let code = muster_cli::run(
        &argv,
        &BTreeMap::new(),
        None,
        &mut std::io::empty(),
        &mut out,
        &mut errors,
    );
    Ran {
        code,
        out: String::from_utf8_lossy(&out).into_owned(),
        errors: String::from_utf8_lossy(&errors).into_owned(),
    }
}

/// Asks a window what it is showing, which is the smallest request there is.
///
/// Which request hardly matters: what a failure means is decided by how far the exchange got,
/// not by what was being asked for.
fn asked(socket: &Path) -> Result<Response, muster_cli::Trouble> {
    let request = Request { payload: Some(request::Payload::ReadWindow(ReadWindow {})) };
    dial::ask_within(&request, Some(&socket.to_string_lossy()), &BTreeMap::new(), BRIEFLY)
}

fn socket_at(named: &str) -> PathBuf {
    let root = PathBuf::from("/tmp/muster-cli").join("exit-codes");
    std::fs::create_dir_all(&root).expect("/tmp is writable");
    let path = root.join(format!("{named}.sock"));
    let _ = std::fs::remove_file(&path);
    path
}
