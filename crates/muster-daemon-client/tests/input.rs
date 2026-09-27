//! The input connection the app's keystrokes will travel, against the real daemon.

use muster_daemon_client::input::Input;
use muster_daemon_proto::{self as proto, input_event};
use muster_harness::requests::*;
use muster_harness::{Daemon, until};

/// libghostty's key code for `a`.
const KEY_A: u32 = 20;

fn event(pane: &str, input: input_event::Input) -> proto::InputEvent {
    proto::InputEvent { pane: pane.to_string(), input: Some(input) }
}

fn sent(text: &str, enter: bool) -> input_event::Input {
    input_event::Input::Send(input_event::Send { text: text.to_string(), enter })
}

fn open(daemon: &Daemon) -> Input {
    Input::open(daemon.socket_path(), "test", Box::new(|| {}))
        .expect("the daemon welcomes an input connection")
}

fn running(name: &str, command: &str) -> proto::pane_request::Create {
    proto::pane_request::Create {
        command: Some(command.to_string()),
        ..create(name, in_new_tab(name))
    }
}

#[test]
fn a_send_and_a_key_reach_the_program() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, running("p1", "cat"));
    let input = open(&daemon);

    input.send(event("p1", sent("hello", true))).unwrap();
    until_text(&mut control, "p1", "hello\nhello");
    input
        .send(event(
            "p1",
            input_event::Input::Key(input_event::Key {
                action: proto::KeyAction::Press.into(),
                key: KEY_A,
                text: "a".into(),
                ..input_event::Key::default()
            }),
        ))
        .unwrap();
    until_text(&mut control, "p1", "hello\na");
}

#[test]
fn events_arrive_in_the_order_they_were_sent() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    let out = daemon.root().join("typed");
    let ready = daemon.root().join("ready");
    // Raw before anything is sent: a terminal in canonical mode holds about a kilobyte of an
    // unfinished line and discards the rest.
    let command = format!("stty raw -echo; echo > {}; cat > {}", ready.display(), out.display());
    make(&mut control, running("p1", &command));
    muster_harness::until_file(&ready, "the program to be ready");
    let input = open(&daemon);

    let expected = (0..1000).map(|n| n.to_string()).collect::<Vec<_>>().join(",") + ",";
    for n in 0..1000 {
        input.send(event("p1", sent(&format!("{n},"), false))).unwrap();
    }
    until(
        "every send to reach the program, in order",
        || std::fs::read_to_string(&out).is_ok_and(|typed| typed == expected),
        || std::fs::read_to_string(&out).unwrap_or_default(),
    );
}

#[test]
fn a_send_to_a_daemon_that_has_gone_returns_at_once() {
    let mut daemon = Daemon::start_built();
    let input = open(&daemon);
    daemon.kill();
    let started = std::time::Instant::now();
    for _ in 0..10_000 {
        let _ = input.send(event("p1", sent("lost", false)));
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
}
