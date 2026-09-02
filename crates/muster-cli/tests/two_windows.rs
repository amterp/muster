//! What `muster` does when nobody said which window, and more than one is listening.
//!
//! No daemon and no app here, deliberately. The decision under test is the CLI's own and is made
//! before anything is dialled - is this a question, and did the caller name a window - and the
//! only thing it needs from the other end is that something answers on two sockets. Standing up
//! two real windows would need two processes, because the seam holds one session each, and would
//! test the same branch through a great deal more machinery.
//!
//! So the far end here is two listeners answering a canned `Window`. That is not a stand-in for a
//! daemon, which this repo does not have: it is a stand-in for a peer *client of this CLI's own
//! protocol*, and the protobuf and the framing it answers with are the real ones from
//! `muster-proto`.

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use muster_proto::{
    Response, RosterChanged, RosterDaemon, RosterPane, RosterTab, Window, frame, response,
};
use prost::Message;

#[test]
fn a_question_nobody_narrowed_is_answered_by_every_window() {
    let scratch = Scratch::new("every");
    let home = scratch.home();
    let first = window(home, 111, "first-pane");
    let second = window(home, 222, "second-pane");

    let (code, out, errors) = run(&["window"], home, None);

    assert_eq!(code, 0, "muster window refused with two windows open: {errors}");
    for expected in [&first, &second, "window 111", "window 222"] {
        assert!(
            out.contains(expected),
            "the answer does not mention {expected}, so one of the two windows is missing \
             from it:\n{out}"
        );
    }
}

#[test]
fn one_window_answers_exactly_as_it_did_before() {
    let scratch = Scratch::new("one");
    let home = scratch.home();
    let only = window(home, 333, "only-pane");

    let (code, out, _) = run(&["window"], home, None);

    assert_eq!(code, 0);
    assert!(out.contains(&only), "the one window's answer is missing its pane:\n{out}");
    // No heading, because there is nothing to tell apart. A script reading one window's output
    // is the case that must not move, and this is the shape of that promise.
    assert!(
        !out.contains("window 333"),
        "one window's answer grew a heading, so every caller reading it has to learn about \
         windows in the plural:\n{out}"
    );
}

#[test]
fn a_caller_inside_a_pane_still_hears_only_its_own_window() {
    let scratch = Scratch::new("pane");
    let home = scratch.home();
    let first = window(home, 444, "first-pane");
    let second = window(home, 555, "second-pane");

    let (code, out, _) = run(&["window"], home, Some(&first_socket(home, 444)));

    assert_eq!(code, 0);
    assert!(out.contains(&first), "the pane's own window is not in the answer:\n{out}");
    assert!(
        !out.contains(&second),
        "a command run inside a pane answered about another window too, which is what \
         $MUSTER_SOCKET exists to prevent:\n{out}"
    );
}

#[test]
fn a_change_with_two_windows_open_still_refuses_and_names_them() {
    let scratch = Scratch::new("write");
    let home = scratch.home();
    window(home, 666, "first-pane");
    window(home, 777, "second-pane");

    let (code, out, errors) = run(&["pane", "new", "--down"], home, None);

    assert_eq!(code, 3, "a change was carried out with nothing saying which window it was for");
    assert!(out.is_empty(), "a refused command wrote to stdout: {out}");
    for expected in ["command-666.sock", "command-777.sock", "--socket"] {
        assert!(
            errors.contains(expected),
            "the refusal does not mention {expected}, so it does not say how to pick one:\n\
             {errors}"
        );
    }
}

#[test]
fn a_program_reading_two_windows_gets_one_object_per_window() {
    let scratch = Scratch::new("json");
    let home = scratch.home();
    window(home, 888, "first-pane");
    window(home, 999, "second-pane");

    let (code, out, _) = run(&["window", "--json"], home, None);

    assert_eq!(code, 0);
    let answered: serde_json::Value =
        serde_json::from_str(&out).unwrap_or_else(|error| panic!("not JSON ({error}): {out}"));
    let windows = answered["windows"].as_array().expect("an object with a windows array");
    assert_eq!(windows.len(), 2, "{out}");
    // Flattened rather than nested, so a filter written against one window's answer reads across
    // several: `.windows[].panes[] | select(...)`.
    for window in windows {
        assert!(window["panes"].is_array(), "a window's row carries no panes: {window}");
        assert!(window["window"].is_string(), "a window's row does not say which window: {window}");
    }
}

/// A home of its own per test, since the CLI finds windows by reading one directory.
///
/// Under `/tmp` rather than under the platform's temporary directory, and named as briefly as
/// this reads: a unix socket path has about a hundred bytes to spend, and macOS hands out
/// `/var/folders/<two>/<long>/T/` which spends most of them before a name is written. The same
/// reason `herdr-harness` picks its own root.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(named: &str) -> Scratch {
        let root = PathBuf::from("/tmp/muster-cli").join(named);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("state")).expect("/tmp is writable");
        Scratch { root }
    }

    fn home(&self) -> &Path {
        &self.root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn first_socket(home: &Path, pid: u32) -> String {
    home.join("state").join(format!("command-{pid}.sock")).to_string_lossy().into_owned()
}

/// A listener answering as a window would, for as long as the test runs.
///
/// Answers every connection rather than one: `survey` dials each socket once per command, and a
/// test that ran two commands against one listener would find the second unanswered.
fn window(home: &Path, pid: u32, pane: &str) -> String {
    let path = first_socket(home, pid);
    let listener = UnixListener::bind(&path).expect("the temporary directory is writable");
    let answer = Response {
        payload: Some(response::Payload::Window(Window {
            roster: Some(RosterChanged {
                daemons: vec![RosterDaemon {
                    daemon_id: "local".to_string(),
                    tabs: vec![RosterTab {
                        daemon_id: "local".to_string(),
                        tab_id: format!("t{pid}"),
                        place: 1,
                        label: format!("tab of {pid}"),
                        panes: vec![RosterPane {
                            daemon_id: "local".to_string(),
                            pane_id: pane.to_string(),
                            place: 1,
                            label: pane.to_string(),
                            on_screen: true,
                            ..RosterPane::default()
                        }],
                        ..RosterTab::default()
                    }],
                }],
                ..RosterChanged::default()
            }),
            ..Window::default()
        })),
    }
    .encode_to_vec();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // Read the request off the socket before answering, because the other end writes and
            // then reads: answering without draining would leave a frame in the buffer and the
            // next command talking to a socket that is out of step.
            let _ = frame::read_frame(&mut stream, frame::LARGEST_MESSAGE);
            let _ = frame::write_frame(&mut stream, &answer);
            let mut drained = Vec::new();
            let _ = stream.read_to_end(&mut drained);
        }
    });
    pane.to_string()
}

fn run(argv: &[&str], home: &Path, in_a_pane: Option<&str>) -> (i32, String, String) {
    let mut environment = BTreeMap::new();
    environment.insert("MUSTER_HOME".to_string(), home.to_string_lossy().into_owned());
    if let Some(socket) = in_a_pane {
        environment.insert("MUSTER_SOCKET".to_string(), socket.to_string());
    }
    let argv: Vec<String> = argv.iter().map(|word| (*word).to_string()).collect();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let code = muster_cli::run(&argv, &environment, &mut out, &mut errors);
    (
        code,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&errors).into_owned(),
    )
}
