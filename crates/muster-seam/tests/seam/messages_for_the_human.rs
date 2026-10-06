//! A message for the human, through a real daemon to the window: it raises exactly one
//! notification, going to what asked lands on the group's transcript, and chatter between agents
//! raises nothing (MIP-4, section 10).

use muster::proto::{
    AttentionChanged, DeleteGroup, Event, FocusAsking, GroupsChanged, LeaveGroup, OpenWindow,
    ReadWindow, Request, Response, Startup, WindowFocus, event, request, response,
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
    assert!(nothing_waits(&mut control), "the human has read it");
    assert_eq!(focus_asking(), muster::proto::Asking::default(), "something still asks");

    // Somebody reading the transcript is the human reading it: a message then asks nothing,
    // and is read.
    assert_ok(&answer(request::Payload::WindowFocus(WindowFocus { focused: true })));
    let before = asked_of("g").len();
    post(&mut control, "a", "@human", "and this");
    let mut watching = daemon.connect();
    until(
        "the window to read what it is showing",
        || nothing_waits(&mut control),
        || format!("the daemon says {:?} waits", snapshot(&mut watching).human),
    );
    assert_eq!(asked_of("g").len(), before, "asked while looking: {:?}", self::asked());
    assert_eq!(keyboard_pane(), transcript, "the transcript was found again, not made again");
}

/// The sidebar's groups: every group the human is in, with what waits there, still listed once
/// they have read it and gone once they leave (MIP-4, Decision 1b).
#[test]
fn the_humans_groups_are_listed_with_what_waits_in_each_until_they_leave() {
    let _turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    GROUPS.lock().expect("a panicking test poisoned the log").take();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    open_window(&daemon);
    group_of_three(&mut control);
    until_groups(&[("g", 0, 0)]);

    post(&mut control, "a", "@human", "need input");
    post(&mut control, "a", "@human", "and this");
    until_groups(&[("g", 2, 2)]);

    let opened = answer(request::Payload::OpenTranscript(muster::proto::OpenTranscript {
        daemon_id: String::new(),
        group: "g".to_string(),
    }));
    assert_ok(&opened);
    until_groups(&[("g", 0, 0)]);

    let leave = Asked::Leave(msg_request::Leave { group: Some("g".to_string()) });
    expect(&mut control, msg(&the_human(), leave), daemon_proto::Outcome::Done);
    until_groups(&[]);
}

/// The sidebar's verbs on a group row: leaving takes the group off the list and leaves it be,
/// deleting takes it off the list and off its daemon, and a refusal comes back in the daemon's
/// words.
#[test]
fn a_group_left_or_deleted_through_the_window_is_no_longer_listed() {
    let _turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    GROUPS.lock().expect("a panicking test poisoned the log").take();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    open_window(&daemon);
    group_of_three(&mut control);
    for (caller, name) in [(the_human(), "@human"), (named("a"), "a")] {
        let join = Asked::Join(msg_request::Join {
            name: Some(name.to_string()),
            group: Some("h".to_string()),
            pull: false,
        });
        expect(&mut control, msg(&caller, join), daemon_proto::Outcome::Done);
    }
    until_groups(&[("g", 0, 0), ("h", 0, 0)]);
    let daemon_id = GROUPS
        .lock()
        .expect("a panicking test poisoned the log")
        .as_ref()
        .map_or(String::new(), |told| told.groups[0].daemon_id.clone());
    let leave = |group: &str| {
        request::Payload::LeaveGroup(LeaveGroup {
            daemon_id: daemon_id.clone(),
            group: group.into(),
        })
    };
    let delete = |group: &str| {
        let daemon_id = daemon_id.clone();
        request::Payload::DeleteGroup(DeleteGroup { daemon_id, group: group.into() })
    };

    assert_ok(&answer(leave("g")));
    until_groups(&[("h", 0, 0)]);
    assert_ok(&answer(delete("h")));
    until_groups(&[]);
    assert_eq!(groups_held(&mut control), ["g"], "g was left, not deleted, and h is gone");

    let refused = answer(delete("nope"));
    let Some(response::Payload::Failure(failure)) = &refused.payload else {
        panic!("a delete of no group answered {refused:?}");
    };
    assert!(failure.reason.contains("nope"), "in the daemon's words: {}", failure.reason);
}

/// The groups the daemon keeps, by name.
fn groups_held(control: &mut Control) -> Vec<String> {
    let asked = msg(&named("a"), Asked::Groups(msg_request::Groups {}));
    let held = expect(control, asked, daemon_proto::Outcome::Done);
    match held.answer.detail {
        Some(daemon_proto::answer::Detail::Msg(daemon_proto::MsgAnswer {
            answer: Some(Answer::Groups(groups)),
            ..
        })) => groups.groups.into_iter().map(|group| group.name).collect(),
        other => panic!("a groups request answered with {other:?}"),
    }
}

/// Waits until the shell was last told exactly these groups, as (group, unread, to you).
fn until_groups(wanted: &[(&str, u64, u64)]) {
    let told = || -> Option<Vec<(String, u64, u64)>> {
        GROUPS.lock().expect("a panicking test poisoned the log").as_ref().map(|told| {
            told.groups
                .iter()
                .map(|group| (group.group.clone(), group.unread, group.to_you))
                .collect()
        })
    };
    let wanted: Vec<(String, u64, u64)> = wanted
        .iter()
        .map(|(group, unread, to_you)| (group.to_string(), *unread, *to_you))
        .collect();
    until(
        "the shell to be told the human's groups",
        || told().as_ref() == Some(&wanted),
        || format!("it was last told {:?}", told()),
    );
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

/// `muster msg open` names no daemon, which means the one on this machine: it opens the group's
/// transcript there, answers with its pane, and a second open goes to the same pane.
#[test]
fn a_transcript_opened_naming_no_daemon_is_the_one_here_and_opens_once() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    open_window(&daemon);
    group_of_three(&mut control);
    let open = || {
        answer(request::Payload::OpenTranscript(muster::proto::OpenTranscript {
            daemon_id: String::new(),
            group: "g".to_string(),
        }))
    };

    let Some(response::Payload::Went(went)) = open().payload else { panic!("not opened") };
    assert_eq!(went.daemon_id, "local");
    assert_eq!(command_of(&mut control, &went.pane_id), "muster msg log --group='g' --follow");
    let Some(response::Payload::Went(again)) = open().payload else { panic!("not opened") };
    assert_eq!(again.pane_id, went.pane_id, "a second open made a second transcript");
}

/// With two daemons on this machine, naming no daemon means the one the config names first,
/// whatever their ids sort as.
#[test]
fn a_transcript_opened_naming_no_daemon_is_the_first_the_config_names() {
    let _turn = muster::testing::fresh_session();
    let home = Daemon::start_built();
    let other = Daemon::start_built();
    let mut control = home.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    open_window_with(&home.muster_config_naming("zz-home", &[("aa-other", &other)]));
    group_of_three(&mut control);

    let opened = answer(request::Payload::OpenTranscript(muster::proto::OpenTranscript {
        daemon_id: String::new(),
        group: "g".to_string(),
    }));
    let Some(response::Payload::Went(went)) = opened.payload else { panic!("{opened:?}") };
    assert_eq!(went.daemon_id, "zz-home");
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
    open_window_with(&daemon.muster_config());
}

fn open_window_with(config: &std::path::Path) {
    for payload in [
        request::Payload::Startup(Startup {
            config_path: config.to_string_lossy().into_owned(),
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

/// Whether the daemon says nothing waits for the human in any group: the groups they are in are
/// still listed, each at 0.
fn nothing_waits(control: &mut Control) -> bool {
    snapshot(control).human.iter().all(|notice| notice.count == 0)
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

/// What the shell was last told of the human's groups.
static GROUPS: Mutex<Option<GroupsChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::AttentionChanged(asked)) => {
            ASKED.lock().expect("a panicking test poisoned the log").push(asked);
        }
        Some(event::Payload::GroupsChanged(groups)) => {
            *GROUPS.lock().expect("a panicking test poisoned the log") = Some(groups);
        }
        _ => {}
    }
}
