//! A message for the human, through a real daemon to the window: it raises exactly one
//! notification, going to what asked shows the group's conversation, and chatter between agents
//! raises nothing (MIP-4, section 10; mip/0009-group-conversation.md, section 1).

use crate::group_messages::{group_of_three, msg, named, post, posted, the_human};
use muster::proto::{
    AskingListChanged, AttentionChanged, DeleteGroup, Event, FocusAsking, GroupsChanged,
    LeaveGroup, OpenWindow, ReadWindow, Request, Response, Startup, WindowFocus, event, request,
    response,
};
use muster_daemon_proto as daemon_proto;
use muster_daemon_proto::msg_answer::{self, Answer};
use muster_daemon_proto::msg_request::{self, Request as Asked};
use muster_harness::requests::{create, expect, in_new_tab, make, snapshot, until_text};
use muster_harness::{Control, Daemon, until};
use prost::Message;
use std::sync::Mutex;

#[test]
fn a_second_message_updates_the_asking_preview_without_moving_it() {
    let _turn = muster::testing::fresh_session();
    ASKING_LISTS.lock().expect("asking lists").clear();
    muster::ffi::muster_set_event_callback(Some(note));
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    open_window(&daemon);
    group_of_three(&mut control);
    post(&mut control, "a", "@human", "first message");
    let latest = || ASKING_LISTS.lock().expect("asking lists").last().cloned();
    until(
        "the first preview",
        || {
            latest().is_some_and(|list| {
                list.entries.iter().any(|entry| entry.preview == "first message")
            })
        },
        || format!("{:?}", latest()),
    );
    let first = latest().expect("asking list").entries[0].clone();
    assert_eq!(first.group, "g");
    assert_eq!(first.alert, "message");
    assert!(first.since_ms > 0);
    post(&mut control, "a", "@human", "second message");
    until(
        "the second preview",
        || {
            latest().is_some_and(|list| {
                list.entries.iter().any(|entry| entry.preview == "second message")
            })
        },
        || format!("{:?}", latest()),
    );
    let second = latest().expect("asking list").entries[0].clone();
    assert_eq!(second.group, first.group);
    assert_eq!(second.since_ms, first.since_ms);
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        panic!("window")
    };
    assert_eq!(window.asking, vec![second]);
}

#[test]
fn a_message_preview_is_fetched_when_notifications_are_disabled() {
    let _turn = muster::testing::fresh_session();
    ASKING_LISTS.lock().expect("asking lists").clear();
    muster::ffi::muster_set_event_callback(Some(note));
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    let config = daemon.muster_config();
    let original = std::fs::read_to_string(&config).expect("config");
    std::fs::write(&config, format!("{original}\n[notifications]\nmessages = false\n"))
        .expect("config");
    open_window_with(&config);
    group_of_three(&mut control);
    post(&mut control, "a", "@human", "quiet message");
    let latest = || ASKING_LISTS.lock().expect("asking lists").last().cloned();
    until(
        "the quiet preview",
        || {
            latest().is_some_and(|list| {
                list.entries.iter().any(|entry| entry.preview == "quiet message")
            })
        },
        || format!("{:?}", latest()),
    );
}

/// The window is open when the message arrives: one banner for it and none for the chatter
/// before it, and going to what asked shows the group's conversation, which is the human reading
/// it, so nothing is left asking.
#[test]
fn a_message_for_the_human_notifies_once_and_lands_on_its_conversation() {
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
    let posted = posted(&mut control, &named("a"), "@human", "need input", &[]);
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

    // ⌘⇧A is pressed in a focused window, and the conversation reads only while it is.
    assert_ok(&answer(request::Payload::WindowFocus(WindowFocus { focused: true })));
    let went = focus_asking();
    assert_eq!(went.group, "g", "went to {went:?}");
    assert_eq!(went.pane_id, "");
    assert_eq!(conversation_shown(), ("local".to_string(), "g".to_string()));
    until(
        "going there to take the banner back",
        || asked_of("g").last().is_some_and(|last| last.state.is_empty()),
        || format!("the window asked {:?}", self::asked()),
    );
    // The banner comes down at once, and the daemon hears the read when it is sent.
    let mut watching = daemon.connect();
    until(
        "the daemon to hear the human read it",
        || nothing_waits(&mut control),
        || format!("the daemon says {:?} waits", snapshot(&mut watching).human),
    );
    assert_eq!(focus_asking(), muster::proto::Asking::default(), "something still asks");

    // The conversation in a focused window is the human reading it: a message then asks
    // nothing, and is read.
    let before = asked_of("g").len();
    post(&mut control, "a", "@human", "and this");
    until(
        "the window to read what it is showing",
        || nothing_waits(&mut control),
        || format!("the daemon says {:?} waits", snapshot(&mut watching).human),
    );
    assert_eq!(asked_of("g").len(), before, "asked while looking: {:?}", self::asked());
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

    assert_ok(&answer(request::Payload::WindowFocus(WindowFocus { focused: true })));
    assert_ok(&answer(show_conversation("")));
    until_groups(&[("g", 0, 0)]);

    let leave = Asked::Leave(msg_request::Leave { group: Some("g".to_string()) });
    expect(&mut control, msg(&the_human(), leave), daemon_proto::Outcome::Done);
    until_groups(&[]);
}

#[test]
fn read_pair_tracks_local_members_and_partner_without_posts() {
    pair_changes(false, false);
}

#[test]
fn read_pair_tracks_remote_members_and_partner_without_posts() {
    pair_changes(true, false);
}

#[test]
fn a_remote_partner_move_is_found_after_the_peer_link_returns() {
    pair_changes(true, true);
}

fn pair_changes(remote: bool, cut_link: bool) {
    let _turn = muster::testing::fresh_session();
    GROUPS.lock().expect("groups").take();
    muster::ffi::muster_set_event_callback(Some(note));
    let home = Daemon::start_detecting();
    let far = Daemon::start_detecting();
    let mut here = home.connect();
    let mut there = far.connect();
    make(&mut here, create("homepane", in_new_tab("hometab")));
    if remote {
        make(&mut there, create("partner1", in_new_tab("partnertab1")));
        make(&mut there, create("partner2", in_new_tab("partnertab2")));
    } else {
        make(&mut here, create("partner1", in_new_tab("partnertab1")));
        make(&mut here, create("partner2", in_new_tab("partnertab2")));
    }
    let partner_daemon = if remote { &far } else { &home };
    partner_daemon.run_agent("partner1");
    partner_daemon.run_agent("partner2");
    let mut logs = home.connect();
    let follow = daemon_proto::session_request::Request::FollowLog(
        daemon_proto::session_request::FollowLog { after: None },
    );
    expect(
        &mut logs,
        daemon_proto::request::Service::Session(daemon_proto::SessionRequest {
            request: Some(follow),
        }),
        daemon_proto::Outcome::Done,
    );
    let mut link = if remote {
        let mut holding = home.connect();
        let peer = msg_request::Peer {
            name: "far".into(),
            socket: far.socket_path().to_string_lossy().into_owned(),
        };
        holding.send(msg(&named("app"), Asked::Peer(peer)));
        let lines = logs.logged_until("msg.peer.linked", std::time::Duration::from_secs(20));
        assert!(lines.iter().any(|line| line.line.contains("msg.peer.linked")), "link failed");
        open_window_with(&home.muster_config_naming("home", &[("far", &far)]));
        Some(holding)
    } else {
        open_window(&home);
        None
    };
    join_pair(&mut here, &the_human(), "@human");
    let partner = |pane: &str| msg_request::Caller {
        as_name: Some("partner".into()),
        pane: Some(pane.into()),
        ..Default::default()
    };
    let control = if remote { &mut there } else { &mut here };
    let joined = join_pair(control, &partner("partner1"), "partner");
    if remote {
        assert!(joined.starts_with("g@"), "remote joined a different group: {joined}");
    }
    let human = if remote { remote_human(control, &partner("partner1")) } else { "@human".into() };
    let sent = posted(control, &partner("partner1"), &human, "initial message", &[]);
    assert!(matches!(sent.answer, Some(Answer::Posted(_))), "post refused: {sent:?}");
    until_groups(&[("g", 1, 1)]);
    expect(
        &mut here,
        msg(&the_human(), Asked::Read(msg_request::Read { group: Some("g".into()) })),
        daemon_proto::Outcome::Done,
    );
    until_groups(&[("g", 0, 0)]);
    until_partner(Some((if remote { "far" } else { "local" }, "partner1")));

    join_pair(&mut here, &named("third"), "third");
    until_partner(None);
    expect(
        &mut here,
        msg(&named("third"), Asked::Leave(msg_request::Leave { group: Some("g".into()) })),
        daemon_proto::Outcome::Done,
    );
    until_partner(Some((if remote { "far" } else { "local" }, "partner1")));

    if cut_link {
        drop(link.take());
        let lines = logs.logged_until("msg.peer.unlinked", std::time::Duration::from_secs(20));
        assert!(lines.iter().any(|line| line.line.contains("msg.peer.unlinked")), "link stayed up");
    }
    quit_partner(partner_daemon);
    let control = if remote { &mut there } else { &mut here };
    expect(
        control,
        msg(&partner("partner2"), Asked::Read(msg_request::Read { group: Some("g".into()) })),
        daemon_proto::Outcome::Done,
    );
    if cut_link {
        until_partner(Some(("far", "partner1")));
        let mut holding = home.connect();
        let peer = msg_request::Peer {
            name: "far".into(),
            socket: far.socket_path().to_string_lossy().into_owned(),
        };
        holding.send(msg(&named("app"), Asked::Peer(peer)));
        let lines = logs.logged_until("msg.peer.linked", std::time::Duration::from_secs(20));
        assert!(
            lines.iter().any(|line| line.line.contains("msg.peer.linked")),
            "link did not return"
        );
        link = Some(holding);
    }
    until_partner(Some((if remote { "far" } else { "local" }, "partner2")));
    drop(link);
}

fn quit_partner(daemon: &Daemon) {
    daemon.type_into("partner1", "quit");
    let mut control = daemon.connect();
    until(
        "the first agent to exit without closing its pane",
        || snapshot(&mut control).panes.iter().any(|p| p.pane == "partner1" && p.agent.is_none()),
        || "the first pane still has an agent".to_string(),
    );
}

fn remote_human(control: &mut Control, caller: &msg_request::Caller) -> String {
    let asked = Asked::Who(msg_request::Who { group: Some("g".into()) });
    let answer = expect(control, msg(caller, asked), daemon_proto::Outcome::Done);
    let Some(daemon_proto::answer::Detail::Msg(daemon_proto::MsgAnswer {
        answer: Some(Answer::Members(members)),
        ..
    })) = answer.answer.detail
    else {
        panic!("Who did not return members")
    };
    members
        .members
        .iter()
        .find(|m| m.name.starts_with("@human@"))
        .unwrap_or_else(|| panic!("remote human missing: {:?}", members.members))
        .name
        .clone()
}

fn join_pair(control: &mut Control, caller: &msg_request::Caller, name: &str) -> String {
    let join = Asked::Join(msg_request::Join {
        name: Some(name.into()),
        group: Some("g".into()),
        pull: false,
    });
    let joined = expect(control, msg(caller, join), daemon_proto::Outcome::Done);
    let Some(daemon_proto::answer::Detail::Msg(daemon_proto::MsgAnswer {
        answer: Some(Answer::Joined(joined)),
        ..
    })) = joined.answer.detail
    else {
        panic!("join did not return a group")
    };
    joined.group.expect("join named a group")
}

fn until_partner(expected: Option<(&str, &str)>) {
    until(
        "the group's partner pane",
        || {
            GROUPS
                .lock()
                .expect("groups")
                .as_ref()
                .and_then(|groups| groups.groups.iter().find(|g| g.group == "g"))
                .is_some_and(|group| {
                    group
                        .partner_pane
                        .as_ref()
                        .map(|pane| (pane.daemon_id.as_str(), pane.pane_id.as_str()))
                        == expected
                })
        },
        || format!("wanted {expected:?}, groups: {:?}", GROUPS.lock().expect("groups")),
    );
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
    let posted = posted(&mut control, &named("a"), "@human", "while you were out", &[]);
    assert_eq!(reached(&posted), [("@human".to_string(), msg_answer::Reach::Waiting)]);

    open_window(&daemon);
    until(
        "the waiting message to ask for the human",
        || !messages_asked().is_empty(),
        || format!("the window asked {:?}", asked()),
    );
    assert_eq!(messages_asked()[0].group, "g");
}

/// `muster msg open` names no daemon, which means the one on this machine: the window shows the
/// group's conversation there.
#[test]
fn a_conversation_shown_naming_no_daemon_is_on_the_daemon_here() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    open_window(&daemon);
    group_of_three(&mut control);

    assert_ok(&answer(show_conversation("")));
    assert_eq!(conversation_shown(), ("local".to_string(), "g".to_string()));
}

/// With two daemons on this machine, naming no daemon means the one the config names first,
/// whatever their ids sort as.
#[test]
fn a_conversation_shown_naming_no_daemon_is_on_the_first_the_config_names() {
    let _turn = muster::testing::fresh_session();
    let home = Daemon::start_built();
    let other = Daemon::start_built();
    let mut control = home.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    open_window_with(&home.muster_config_naming("zz-home", &[("aa-other", &other)]));
    group_of_three(&mut control);

    assert_ok(&answer(show_conversation("")));
    assert_eq!(conversation_shown(), ("zz-home".to_string(), "g".to_string()));
}

/// Shows group `g`'s conversation on `daemon`, or on the one on this machine for an empty one.
fn show_conversation(daemon: &str) -> request::Payload {
    request::Payload::ShowConversation(muster::proto::ShowConversation {
        daemon_id: daemon.to_string(),
        group: "g".to_string(),
    })
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

fn focus_asking() -> muster::proto::Asking {
    match answer(request::Payload::FocusAsking(FocusAsking {})).payload {
        Some(response::Payload::Asking(asking)) => asking,
        other => panic!("the core answered a FocusAsking with {other:?}"),
    }
}

/// What the window's view says: the pane with the keyboard, and the conversation it shows.
fn view() -> muster::proto::ViewChanged {
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        return muster::proto::ViewChanged::default();
    };
    window.view.unwrap_or_default()
}

/// The pane with the keyboard.
fn keyboard_pane() -> String {
    view().keyboard_pane_id
}

/// The daemon and group whose conversation the window shows, both empty when it shows panes.
fn conversation_shown() -> (String, String) {
    let view = view();
    (view.conversation_daemon_id, view.conversation_group)
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
static ASKING_LISTS: Mutex<Vec<AskingListChanged>> = Mutex::new(Vec::new());

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
        Some(event::Payload::AskingListChanged(list)) => {
            ASKING_LISTS.lock().expect("asking lists").push(list);
        }
        _ => {}
    }
}
