//! The banner for a message to the human, through a real daemon (mip/0009-group-conversation.md,
//! section 5): it shows the message fetched from the group's log, a reply goes to the author it
//! shows and reads nothing, a dismissal reads nothing, reading the group elsewhere takes it down,
//! and a reply the daemon refuses or never answers stays in it.

use muster::proto::{
    BannersChanged, DismissBanner, Event, HoldBanner, OpenWindow, ReplyToBanner, Request, Response,
    Startup, event, markdown_block, post_outcome, request, response,
};
use muster_daemon_proto as daemon_proto;
use muster_daemon_proto::msg_answer::{Answer, entry::What};
use muster_daemon_proto::msg_request::{self, Request as Asked};
use muster_daemon_proto::request::Service;
use muster_harness::requests::{create, expect, in_new_tab, make, snapshot, until_text};
use muster_harness::{Control, Daemon, until};
use prost::Message;
use std::sync::Mutex;

#[test]
fn a_message_for_the_human_raises_a_banner_showing_it_and_the_next_replaces_it() {
    let (_turn, _daemon, mut control) = window_on_a_group_of_three();

    let first = post(&mut control, "a", "@human", "**need** input on the lexer");
    let banner = until_banner(|banner| banner.seq == first);
    assert_eq!((banner.group.as_str(), banner.author.as_str()), ("g", "a"));
    assert!(banner.replyable, "a message from an agent takes a reply");
    let Some(markdown_block::Kind::Paragraph(paragraph)) = &banner.body[0].kind else {
        panic!("the body is not a paragraph: {:?}", banner.body);
    };
    let runs: Vec<(&str, bool)> =
        paragraph.runs.iter().map(|run| (run.text.as_str(), run.strong)).collect();
    assert_eq!(runs, [("need", true), (" input on the lexer", false)], "Markdown, parsed");

    let second = post(&mut control, "b", "@human", "and mine");
    let banner = until_banner(|banner| banner.seq == second);
    assert_eq!(banner.author, "b", "one banner per group, showing its newest message");
    assert_eq!(banners().shown.len(), 1);
}

#[test]
fn a_reply_goes_to_the_author_the_banner_shows_and_leaves_the_group_unread() {
    let (_turn, _daemon, mut control) = window_on_a_group_of_three();
    let asked = post(&mut control, "a", "@human", "ok to merge?");
    until_banner(|banner| banner.seq == asked);

    assert_ok(&answer(hold(asked, true)));
    let since = post(&mut control, "b", "@human", "wait, the build is red");
    let held = until_banner(|banner| banner.waiting_author == "b");
    assert_eq!((held.seq, held.author.as_str()), (asked, "a"), "a held banner keeps its message");

    let replied = answer(reply(asked, "yes"));
    let Some(response::Payload::Ok(ok)) = &replied.payload else {
        panic!("the reply answered {replied:?}");
    };
    assert_eq!(ok.note, "nobody live heard it", "a joined with no inbox, so nothing woke it");
    let entry = last_entry(&mut control);
    assert_eq!(
        (entry.0.as_str(), entry.1.as_slice(), entry.2.as_str()),
        ("@human", &["a".to_string()][..], "yes")
    );

    let next = until_banner(|banner| banner.seq == since);
    assert_eq!(next.author, "b", "the message that waited behind the reply takes the banner");
    assert_eq!(unread(&mut control), 2, "a reply reads nothing");
}

#[test]
fn a_dismissed_banner_reads_nothing_and_the_groups_next_message_raises_one_again() {
    let (_turn, _daemon, mut control) = window_on_a_group_of_three();
    let first = post(&mut control, "a", "@human", "first");
    until_banner(|banner| banner.seq == first);

    assert_ok(&answer(request::Payload::DismissBanner(DismissBanner {
        daemon_id: "local".to_string(),
        group: "g".to_string(),
        seq: first,
    })));
    until(
        "the banner to come down",
        || banners().shown.is_empty(),
        || format!("the shell was told {:?}", banners()),
    );
    assert_eq!(unread(&mut control), 1, "a dismissal reads nothing");

    let second = post(&mut control, "a", "@human", "second");
    until_banner(|banner| banner.seq == second);
}

#[test]
fn reading_the_group_from_a_shell_takes_its_banner_down() {
    let (_turn, _daemon, mut control) = window_on_a_group_of_three();
    let first = post(&mut control, "a", "@human", "first");
    until_banner(|banner| banner.seq == first);

    let read = Asked::Read(msg_request::Read { group: Some("g".to_string()) });
    expect(&mut control, msg(&the_human(), read), daemon_proto::Outcome::Done);
    until(
        "the banner to come down once the human read the group",
        || banners().shown.is_empty(),
        || format!("the shell was told {:?}", banners()),
    );
}

#[test]
fn a_reply_the_daemon_refuses_stays_in_the_banner_with_its_words() {
    let (_turn, _daemon, mut control) = window_on_a_group_of_three();
    let asked = post(&mut control, "a", "@human", "ok to merge?");
    until_banner(|banner| banner.seq == asked);
    let leave = Asked::Leave(msg_request::Leave { group: Some("g".to_string()) });
    expect(&mut control, msg(&named("a"), leave), daemon_proto::Outcome::Done);

    let replied = answer(reply(asked, "yes"));
    let Some(response::Payload::Failure(failure)) = &replied.payload else {
        panic!("a reply to an author who left answered {replied:?}");
    };
    let banner = until_banner(|banner| banner.sent.is_some());
    let sent = banner.sent.unwrap_or_default();
    assert_eq!(sent.state(), post_outcome::State::Refused);
    assert_eq!((sent.body.as_str(), sent.said.as_str()), ("yes", failure.reason.as_str()));
    assert!(banner.replyable, "the human can send it again once they have fixed what was wrong");
}

#[test]
fn a_reply_whose_answer_never_came_is_found_in_the_log_and_not_sent_twice() {
    // Takes the client's whole patience, ten seconds, because an answer the window has stopped
    // waiting for is the case under test and nothing shortens that wait from outside.
    let _turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    BANNERS.lock().expect("a panicking test poisoned the log").take();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    let relay = daemon.withholding_answers_where(posts);
    open_window_with(&relay.muster_config());
    group_of_three(&mut control);
    let asked = post(&mut control, "a", "@human", "ok to merge?");
    until_banner(|banner| banner.seq == asked);

    let replied = answer(reply(asked, "yes"));
    let Some(response::Payload::Ok(ok)) = &replied.payload else {
        panic!("a reply kept but never answered, then found in the log, answered {replied:?}");
    };
    assert!(ok.note.contains("never answered"), "and says so: {}", ok.note);
    let human_posts = entries(&mut control)
        .into_iter()
        .filter(|(author, _, body)| author == "@human" && body == "yes")
        .count();
    assert_eq!(human_posts, 1, "posted once and not sent again");
    drop(relay);
}

/// A post as the human: the request whose answer the relay withholds.
fn posts(request: &daemon_proto::Request) -> bool {
    matches!(
        &request.service,
        Some(Service::Msg(daemon_proto::MsgRequest { caller: Some(caller), request: Some(Asked::Post(_)) }))
            if caller.as_name.is_none()
    )
}

/// A window on a daemon holding `g`, with the human, `a` and `b` in it. The turn comes back to be
/// held for the whole test.
fn window_on_a_group_of_three() -> (muster::testing::Turn, Daemon, Control) {
    let turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    BANNERS.lock().expect("a panicking test poisoned the log").take();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    open_window_with(&daemon.muster_config());
    group_of_three(&mut control);
    (turn, daemon, control)
}

fn hold(seq: u64, held: bool) -> request::Payload {
    request::Payload::HoldBanner(HoldBanner {
        daemon_id: "local".to_string(),
        group: "g".to_string(),
        seq,
        held,
    })
}

fn reply(seq: u64, body: &str) -> request::Payload {
    request::Payload::ReplyToBanner(ReplyToBanner {
        daemon_id: "local".to_string(),
        group: "g".to_string(),
        seq,
        body: body.to_string(),
    })
}

/// Waits for the shell to be told of a banner for `g` that `wanted` picks out, and returns it.
fn until_banner(wanted: impl Fn(&muster::proto::Banner) -> bool) -> muster::proto::Banner {
    let found = || banners().shown.into_iter().find(|banner| banner.group == "g" && wanted(banner));
    until(
        "the shell to be told of the banner",
        || found().is_some(),
        || format!("the shell was told {:?}", banners()),
    );
    found().unwrap_or_default()
}

fn banners() -> BannersChanged {
    BANNERS.lock().expect("a panicking test poisoned the log").clone().unwrap_or_default()
}

/// How many messages wait for the human in `g`, as the daemon says.
fn unread(control: &mut Control) -> u64 {
    snapshot(control).human.iter().find(|notice| notice.group == "g").map_or(0, |n| n.count)
}

/// Every message in `g`: author, recipients, body.
fn entries(control: &mut Control) -> Vec<(String, Vec<String>, String)> {
    let log = Asked::Log(msg_request::Log { group: "g".to_string(), ..Default::default() });
    let logged = expect(control, msg(&named("b"), log), daemon_proto::Outcome::Done);
    let Some(daemon_proto::answer::Detail::Msg(daemon_proto::MsgAnswer {
        answer: Some(Answer::Entries(entries)),
        ..
    })) = logged.answer.detail
    else {
        panic!("a log answered {:?}", logged.answer);
    };
    entries
        .groups
        .into_iter()
        .flat_map(|group| group.entries)
        .filter_map(|entry| match entry.what {
            Some(What::Message(message)) => Some((message.author, message.to, message.body)),
            _ => None,
        })
        .collect()
}

fn last_entry(control: &mut Control) -> (String, Vec<String>, String) {
    entries(control).pop().expect("the group holds a message")
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

/// Posts as `from` to `to` in `g`, having read what waits there first as an agent does, and
/// returns the message's number.
fn post(control: &mut Control, from: &str, to: &str, body: &str) -> u64 {
    let read = Asked::Read(msg_request::Read { group: Some("g".to_string()) });
    control.ask(msg(&named(from), read));
    let asked = Asked::Post(msg_request::Post {
        group: Some("g".to_string()),
        to: vec![to.to_string()],
        body: body.to_string(),
        ..Default::default()
    });
    let posted = control.ask(msg(&named(from), asked));
    match posted.answer.detail {
        Some(daemon_proto::answer::Detail::Msg(daemon_proto::MsgAnswer {
            answer: Some(Answer::Posted(posted)),
            ..
        })) => posted.seq,
        other => panic!("a post answered with {other:?}: {}", posted.answer.reason),
    }
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

/// What the shell was last told of the banners.
static BANNERS: Mutex<Option<BannersChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::BannersChanged(told)) = event.payload {
        *BANNERS.lock().expect("a panicking test poisoned the log") = Some(told);
    }
}
