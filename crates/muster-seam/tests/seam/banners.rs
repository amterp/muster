//! The banner for a message to the human, through a real daemon (mip/0009-group-conversation.md,
//! section 5): it shows the message fetched from the group's log, a reply goes to the author it
//! shows and reads nothing, a dismissal reads nothing, reading the group elsewhere takes it down,
//! and a reply the daemon refuses or never answers stays in it.

use crate::group_messages::{
    entries, group_of_three, log, msg, named, post, posted, the_human, unread,
};
use muster::proto::{
    BannersChanged, DismissBanner, Event, HoldBanner, OpenWindow, PickBannerAnswer, ReplyToBanner,
    Request, Response, Startup, event, markdown_block, post_outcome, request, response,
};
use muster_daemon_proto as daemon_proto;
use muster_daemon_proto::msg_answer::{Answer, entry::What};
use muster_daemon_proto::msg_request::{self, Request as Asked};
use muster_daemon_proto::request::Service;
use muster_harness::requests::{create, expect, in_new_tab, make, until_text};
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

#[test]
fn a_questions_banner_draws_its_answers_and_a_reply_sends_the_pick_with_its_note() {
    let (_turn, _daemon, mut control) = window_on_a_group_of_three();
    let asked = ask(&mut control, "a", "run the contract tier?", &["Run it now", "Skip it"]);
    let banner = until_banner(|banner| banner.seq == asked);
    assert_eq!(banner.answers, ["Run it now", "Skip it"]);

    assert_ok(&answer(pick(asked, Some(1))));
    let picked = until_banner(|banner| banner.picked.is_some());
    assert_eq!(picked.picked, Some(1));
    assert_ok(&answer(reply(asked, "after lunch")));

    let (author, to, body) = last_entry(&mut control);
    assert_eq!((author.as_str(), to.as_slice()), ("@human", &["a".to_string()][..]));
    assert_eq!(body, "Answer: Skip it\n\nafter lunch", "the answer above the note");
    let answering = msg_request::Answering { question: asked, answer: "Skip it".to_string() };
    assert_eq!(last_answering(&mut control), Some(answering), "and named as the post's field");
}

#[test]
fn a_picked_answer_holds_the_banner_and_a_refused_reply_keeps_it_with_the_note() {
    let (_turn, _daemon, mut control) = window_on_a_group_of_three();
    let asked = ask(&mut control, "a", "ok to merge?", &["Merge", "Hold off"]);
    until_banner(|banner| banner.seq == asked);
    assert_ok(&answer(pick(asked, Some(0))));
    post(&mut control, "b", "@human", "the build is red");
    let held = until_banner(|banner| banner.waiting_author == "b");
    assert_eq!((held.seq, held.picked), (asked, Some(0)), "a pick holds the banner");

    // Answered from a shell meanwhile, so the daemon refuses a second answer.
    let elsewhere = Asked::Post(msg_request::Post {
        group: Some("g".to_string()),
        to: vec!["a".to_string()],
        body: "Answer: Hold off".to_string(),
        answering: Some(msg_request::Answering { question: asked, answer: "Hold off".to_string() }),
        ..Default::default()
    });
    expect(&mut control, msg(&the_human(), elsewhere), daemon_proto::Outcome::Done);
    let replied = answer(reply(asked, "after lunch"));
    let Some(response::Payload::Failure(failure)) = &replied.payload else {
        panic!("a second answer to a question answered {replied:?}");
    };
    assert!(failure.reason.contains("answered already"), "{}", failure.reason);
    let banner = until_banner(|banner| banner.sent.is_some());
    let sent = banner.sent.clone().unwrap_or_default();
    assert_eq!(sent.state(), post_outcome::State::Refused);
    assert_eq!(sent.body, "after lunch", "the note the human wrote, not the body made from it");
    assert_eq!(banner.picked, Some(0), "and the pick stays with it");
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

fn pick(seq: u64, answer: Option<u32>) -> request::Payload {
    request::Payload::PickBannerAnswer(PickBannerAnswer {
        daemon_id: "local".to_string(),
        group: "g".to_string(),
        seq,
        answer,
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

fn last_entry(control: &mut Control) -> (String, Vec<String>, String) {
    entries(control).pop().expect("the group holds a message")
}

/// The answer the last message in `g` picks, if it picks one.
fn last_answering(control: &mut Control) -> Option<msg_request::Answering> {
    match log(control).pop()?.what {
        Some(What::Message(message)) => message.answering,
        _ => None,
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

/// Asks the human a question as `from`, offering `answers`, and returns its number.
fn ask(control: &mut Control, from: &str, body: &str, answers: &[&str]) -> u64 {
    match posted(control, &named(from), "@human", body, answers).answer {
        Some(Answer::Posted(posted)) => posted.seq,
        other => panic!("a question answered with {other:?}"),
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
