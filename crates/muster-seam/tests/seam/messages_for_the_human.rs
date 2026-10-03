//! A message for the human, through a real daemon to the window: it raises exactly one
//! notification, going to what asked lands on the group's transcript, and chatter between agents
//! raises nothing (MIP-4, section 10).

use muster::proto::{
    AttentionChanged, Event, FocusAsking, OpenWindow, ReadWindow, Request, Response, Startup,
    WindowFocus, event, request, response,
};
use muster_daemon_proto as daemon_proto;
use muster_daemon_proto::msg_answer::{self, Answer};
use muster_daemon_proto::msg_request::{self, Request as Asked};
use muster_daemon_proto::request::Service;
use muster_harness::requests::{create, expect, in_new_tab, make, snapshot, until_text};
use muster_harness::{Control, Daemon, until};
use prost::Message;
use std::sync::Mutex;

/// The window is open when the message arrives: one banner for it and none for the chatter
/// before it, and going to what asked opens the group's transcript, which is the human reading
/// it, so nothing is left asking.
#[test]
fn a_message_for_the_human_notifies_once_and_lands_on_its_transcript() {
    let _turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    ASKED.lock().expect("a panicking test poisoned the log").clear();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    open_window(&daemon);
    group_of_three(&mut control);

    post(&mut control, "a", "b", "the lexer is yours");
    let posted = post(&mut control, "a", "@human", "need input");
    assert_eq!(reached(&posted), [("@human".to_string(), msg_answer::Reach::Woken)]);
    until(
        "the message to ask for the human",
        || !messages_asked().is_empty(),
        || format!("the window asked {:?}", asked()),
    );
    let asked = messages_asked();
    assert_eq!(asked.len(), 1, "one banner, and none for the chatter: {:?}", self::asked());
    assert_eq!((asked[0].group.as_str(), asked[0].count), ("g", 1));
    assert_eq!(asked[0].from, ["a"]);
    assert_eq!(asked[0].pane_id, "", "a message asks by its group, not by a pane");

    let went = focus_asking();
    assert_eq!(went.group, "g", "went to {went:?}");
    assert_eq!(went.pane_id, "");
    let transcript = keyboard_pane();
    assert_eq!(command_of(&mut control, &transcript), "muster msg log --group='g' --follow");
    until(
        "going there to take the banner back",
        || asked_of("g").last().is_some_and(|last| last.state.is_empty()),
        || format!("the window asked {:?}", self::asked()),
    );
    assert!(snapshot(&mut control).human.is_empty(), "the human has read it");
    assert_eq!(focus_asking(), muster::proto::Asking::default(), "something still asks");

    // Somebody reading the transcript is the human reading it: a message then asks nothing,
    // and is read.
    assert_ok(&answer(request::Payload::WindowFocus(WindowFocus { focused: true })));
    let before = asked_of("g").len();
    post(&mut control, "a", "@human", "and this");
    let mut watching = daemon.connect();
    until(
        "the window to read what it is showing",
        || snapshot(&mut control).human.is_empty(),
        || format!("the daemon says {:?} waits", snapshot(&mut watching).human),
    );
    assert_eq!(asked_of("g").len(), before, "asked while looking: {:?}", self::asked());
    assert_eq!(keyboard_pane(), transcript, "the transcript was found again, not made again");
}

/// Messages for the human wait while no window is open, and notify when one opens.
#[test]
fn a_message_sent_while_no_window_was_open_notifies_when_one_opens() {
    let _turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    ASKED.lock().expect("a panicking test poisoned the log").clear();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    group_of_three(&mut control);
    let posted = post(&mut control, "a", "@human", "while you were out");
    assert_eq!(reached(&posted), [("@human".to_string(), msg_answer::Reach::Waiting)]);

    open_window(&daemon);
    until(
        "the waiting message to ask for the human",
        || !messages_asked().is_empty(),
        || format!("the window asked {:?}", asked()),
    );
    assert_eq!(messages_asked()[0].group, "g");
}

/// A group's name reaches a shell only as a group's name: one that could be read as a command
/// gets no transcript, rather than a tab whose shell runs it.
#[test]
fn a_group_name_that_is_a_command_opens_nothing() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    open_window(&daemon);

    for group in ["x'; touch owned; '", "x\ntouch owned", "a b", "-x\\y"] {
        let opened = answer(request::Payload::OpenTranscript(muster::proto::OpenTranscript {
            daemon_id: "local".to_string(),
            group: group.to_string(),
        }));
        assert!(
            matches!(opened.payload, Some(response::Payload::Failure(_))),
            "{group:?} was answered with {opened:?}"
        );
    }
    let commands: Vec<String> =
        snapshot(&mut control).panes.iter().filter_map(|pane| pane.command.clone()).collect();
    assert!(commands.is_empty(), "a pane was made to run {commands:?}");
}

/// The human, `a` and `b` in group `g`.
fn group_of_three(control: &mut Control) {
    for (caller, name) in [(the_human(), "@human"), (named("a"), "a"), (named("b"), "b")] {
        let join = Asked::Join(msg_request::Join {
            name: Some(name.to_string()),
            group: Some("g".to_string()),
            pull: false,
        });
        expect(control, msg(&caller, join), daemon_proto::Outcome::Done);
    }
}

fn open_window(daemon: &Daemon) {
    for payload in [
        request::Payload::Startup(Startup {
            config_path: daemon.muster_config().to_string_lossy().into_owned(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        assert_ok(&answer(payload));
    }
    until(
        "the window to show a tab",
        || !keyboard_pane().is_empty(),
        || "the window shows nothing".to_string(),
    );
}

fn the_human() -> msg_request::Caller {
    msg_request::Caller::default()
}

fn named(name: &str) -> msg_request::Caller {
    msg_request::Caller { as_name: Some(name.to_string()), ..msg_request::Caller::default() }
}

fn msg(caller: &msg_request::Caller, asked: Asked) -> Service {
    Service::Msg(daemon_proto::MsgRequest { caller: Some(caller.clone()), request: Some(asked) })
}

fn post(control: &mut Control, from: &str, to: &str, body: &str) -> daemon_proto::MsgAnswer {
    let asked = Asked::Post(msg_request::Post {
        group: Some("g".to_string()),
        to: vec![to.to_string()],
        body: body.to_string(),
        urgent: false,
    });
    let posted = control.ask(msg(&named(from), asked));
    match posted.answer.detail {
        Some(daemon_proto::answer::Detail::Msg(answer)) => answer,
        other => panic!("a post answered with {other:?}: {}", posted.answer.reason),
    }
}

fn reached(posted: &daemon_proto::MsgAnswer) -> Vec<(String, msg_answer::Reach)> {
    match &posted.answer {
        Some(Answer::Posted(posted)) => {
            posted.reached.iter().map(|reached| (reached.name.clone(), reached.reach())).collect()
        }
        other => panic!("a post answered with {other:?}"),
    }
}

/// What the pane runs, as its daemon has it.
fn command_of(control: &mut Control, pane: &str) -> String {
    let snapshot = snapshot(control);
    let record = snapshot.panes.iter().find(|record| record.pane == pane);
    record.and_then(|record| record.command.clone()).unwrap_or_default()
}

fn focus_asking() -> muster::proto::Asking {
    match answer(request::Payload::FocusAsking(FocusAsking {})).payload {
        Some(response::Payload::Asking(asking)) => asking,
        other => panic!("the core answered a FocusAsking with {other:?}"),
    }
}

/// The pane with the keyboard.
fn keyboard_pane() -> String {
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        return String::new();
    };
    let view = window.view.unwrap_or_default();
    view.regions
        .iter()
        .find(|region| region.region_id == view.focused_region)
        .map(|region| region.pane_id.clone())
        .unwrap_or_default()
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    Response::decode(muster::dispatch(&bytes).as_slice()).expect("a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}

fn asked() -> Vec<AttentionChanged> {
    ASKED.lock().expect("a panicking test poisoned the log").clone()
}

/// Every banner raised for a message, in order.
fn messages_asked() -> Vec<AttentionChanged> {
    asked().into_iter().filter(|asked| asked.state == "message").collect()
}

/// Everything said about one group, raised or taken back.
fn asked_of(group: &str) -> Vec<AttentionChanged> {
    asked().into_iter().filter(|asked| asked.group == group).collect()
}

static ASKED: Mutex<Vec<AttentionChanged>> = Mutex::new(Vec::new());

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::AttentionChanged(asked)) = event.payload {
        ASKED.lock().expect("a panicking test poisoned the log").push(asked);
    }
}
