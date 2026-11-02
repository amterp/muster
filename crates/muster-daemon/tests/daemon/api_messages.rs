//! The API's messages topic and `msg.history` (MIP-10, sections 3 and 4): every group this daemon
//! keeps, its changes and posts as events, and its log in pages, read by a program without moving
//! anybody's place.

use std::collections::BTreeMap;

use crate::attention::{DONE, a_message_for_the_human, join, msg, named, the_human};
use crate::support::*;
use muster_api::connection::LARGEST_MESSAGE;
use prost::Message;
use proto::msg_request::{self, Request as Asked};

fn post(control: &mut Control, caller: &msg_request::Caller, post: msg_request::Post) {
    expect(control, msg(caller, Asked::Post(post)), DONE);
}

fn said(group: &str, body: &str) -> msg_request::Post {
    msg_request::Post {
        group: Some(group.to_string()),
        body: body.to_string(),
        ..Default::default()
    }
}

fn group<'a>(snapshot: &'a api::Snapshot, name: &str) -> &'a api::Group {
    let found = snapshot.groups.iter().find(|group| group.name == name);
    found.unwrap_or_else(|| panic!("no group {name} in {:?}", snapshot.groups))
}

/// The next event `pick` takes, passing over the others.
fn next<T>(api: &mut Api, what: &str, pick: impl Fn(api::event::Event) -> Option<T>) -> T {
    let found = api.event_matching(what, |event| event.event.clone().and_then(&pick).is_some());
    found.event.and_then(pick).expect("matched")
}

fn next_logged(api: &mut Api) -> api::Entry {
    next(api, "an entry", |event| match event {
        api::event::Event::Logged(logged) => logged.entry,
        _ => None,
    })
}

fn places_of(snapshot: &api::Snapshot, group: &str) -> BTreeMap<String, u64> {
    let found = snapshot.places.iter().find(|places| places.group == group);
    found.unwrap_or_else(|| panic!("no places for {group} in {:?}", snapshot.places)).places.clone()
}

fn next_places(api: &mut Api) -> BTreeMap<String, u64> {
    next(api, "where each member has read", |event| match event {
        api::event::Event::Places(places) => Some(places.places),
        _ => None,
    })
}

fn read(control: &mut Control, caller: &msg_request::Caller) {
    let read = Asked::Read(msg_request::Read { group: Some("g".to_string()) });
    expect(control, msg(caller, read), DONE);
}

fn body(entry: &api::Entry) -> Option<&str> {
    match &entry.what {
        Some(api::entry::What::Post(post)) => Some(&post.body),
        _ => None,
    }
}

/// Every entry of `group` after `after` through `through`, page by page, with the pages' answers.
fn fetch(
    api: &mut Api,
    group: &str,
    after: u64,
    through: u64,
) -> (Vec<api::Entry>, Vec<api::Answer>) {
    let (mut entries, mut answers) = (Vec::new(), Vec::new());
    let mut through = through;
    loop {
        let asked = api.ask(history(group, after, through));
        assert_eq!(asked.outcome(), api::Outcome::Done, "{:?}", asked.answer);
        let page = asked.page().clone();
        answers.push(asked.answer);
        let first = page.entries.first().map(|entry| entry.number);
        entries.splice(0..0, page.entries);
        match first {
            Some(first) if page.more => through = first - 1,
            _ => return (entries, answers),
        }
    }
}

fn numbers(entries: &[api::Entry]) -> Vec<u64> {
    entries.iter().map(|entry| entry.number).collect()
}

#[test]
fn the_snapshot_holds_every_group_without_a_body() {
    let daemon = daemon();
    let mut control = daemon.connect();
    a_message_for_the_human(&mut control);
    let mut program = daemon.api();
    let source = program.welcome().machine.clone().and_then(|machine| machine.source);
    let asked = program.subscribe(&[api::Topic::Messages]);
    let snapshot = asked.snapshot();

    let g = group(snapshot, "g");
    assert_eq!(g.members, ["@human", "a"]);
    let home = g.home.clone().expect("a home");
    assert_eq!(Some(home.daemon), source.map(|source| source.daemon), "kept by this daemon");
    assert!(!home.machine.is_empty());
    assert!(!g.paused);
    let policy = g.policy.clone().expect("a policy");
    assert_eq!(policy.membership, ["*"]);
    assert!(g.human_place.is_some_and(|place| place < g.latest), "{g:?}");
    assert_eq!(g.held_after, 0);
    assert!(!format!("{snapshot:?}").contains("need input"), "a snapshot carries no body");
}

/// A group made, posted to, joined, paused and deleted, each as its event, in order.
#[test]
fn each_change_to_a_group_is_an_event() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let mut program = daemon.api();
    program.subscribe(&[api::Topic::Messages]);

    join(&mut control, &named("a"), "a", "g");
    let made = next(&mut program, "the group made", |event| match event {
        api::event::Event::GroupMade(made) => made.group,
        _ => None,
    });
    assert_eq!((made.name.as_str(), made.members.as_slice()), ("g", &["a".to_string()][..]));
    let first = [next_logged(&mut program), next_logged(&mut program)];
    assert_eq!(numbers(&first), [made.latest + 1, made.latest + 2], "the entries it was made with");
    assert!(matches!(first[0].what, Some(api::entry::What::Made(_))), "{first:?}");

    post(&mut control, &named("a"), said("g", "hello"));
    let posted = next_logged(&mut program);
    assert_eq!(body(&posted), Some("hello"), "a post comes with its body");
    assert_eq!(posted.number, made.latest + 3);

    join(&mut control, &named("b"), "b", "g");
    let joined = next_logged(&mut program);
    let who = match joined.what {
        Some(api::entry::What::Joined(joined)) => joined.who,
        other => panic!("{other:?}"),
    };
    assert_eq!(who, "b");
    let changed = next(&mut program, "the members changed", |event| match event {
        api::event::Event::GroupChanged(changed) => changed.group,
        _ => None,
    });
    assert_eq!(changed.members, ["a", "b"]);

    let pause = Asked::Pause(msg_request::Pause { group: "g".to_string() });
    expect(&mut control, msg(&named("a"), pause), DONE);
    let paused = next(&mut program, "the group paused", |event| match event {
        api::event::Event::GroupChanged(changed) => changed.group,
        _ => None,
    });
    assert!(paused.paused);

    let delete = Asked::GroupDelete(msg_request::GroupDelete { group: "g".to_string() });
    expect(&mut control, msg(&named("a"), delete), DONE);
    let deleted = next(&mut program, "the group deleted", |event| match event {
        api::event::Event::GroupDeleted(deleted) => Some(deleted.group),
        _ => None,
    });
    assert_eq!(deleted, "g");
}

/// The card's first proof: a program that misses posts while disconnected fetches only what it
/// lacks, and then hears the next post live, with no number missing or repeated.
#[test]
fn a_program_that_reconnects_fetches_only_what_it_lacks_and_misses_nothing() {
    let daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &named("a"), "a", "g");
    let mut program = daemon.api();
    program.subscribe(&[api::Topic::Messages]);
    let mut seen = 0;
    for each in 0..3 {
        post(&mut control, &named("a"), said("g", &format!("before {each}")));
        seen = next_logged(&mut program).number;
    }
    drop(program);
    for each in 0..5 {
        post(&mut control, &named("a"), said("g", &format!("while away {each}")));
    }

    let mut again = daemon.api();
    let latest = group(again.subscribe(&[api::Topic::Messages]).snapshot(), "g").latest;
    let (fetched, _) = fetch(&mut again, "g", seen, latest);
    assert_eq!(numbers(&fetched), (seen + 1..=latest).collect::<Vec<_>>());
    let bodies: Vec<_> = fetched.iter().filter_map(body).collect();
    assert_eq!(bodies.first(), Some(&"while away 0"));

    post(&mut control, &named("a"), said("g", "after"));
    let live = next_logged(&mut again);
    assert_eq!((live.number, body(&live)), (latest + 1, Some("after")));
}

/// The card's second proof: a backlog of posts of a megabyte each comes in several pages, each
/// in a frame under the limit, together holding every entry.
#[test]
fn a_backlog_of_megabyte_posts_arrives_in_pages_under_the_frame_limit() {
    let daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &named("a"), "a", "g");
    for each in 0..10 {
        post(&mut control, &named("a"), said("g", &format!("{each}{}", "x".repeat(1_000_000))));
    }
    let mut program = daemon.api();
    let latest = group(program.subscribe(&[api::Topic::Messages]).snapshot(), "g").latest;

    let (entries, answers) = fetch(&mut program, "g", 0, latest);
    assert_eq!(numbers(&entries), (1..=latest).collect::<Vec<_>>());
    assert_eq!(entries.iter().filter_map(body).count(), 10);
    assert!(answers.len() >= 3, "{} pages", answers.len());
    for answer in answers {
        let message =
            api::DaemonMessage { message: Some(api::daemon_message::Message::Answer(answer)) };
        assert!(message.encoded_len() < LARGEST_MESSAGE as usize, "{}", message.encoded_len());
    }
}

/// The card's third proof, and the rule that the two topics never disagree: the human reading at
/// the desk moves their place, in the same event that empties what waits, and every unread state
/// a program is sent lists nothing at or before its place.
#[test]
fn a_read_at_the_desk_moves_the_humans_place_and_what_waits_with_it() {
    let daemon = daemon();
    let mut control = daemon.connect();
    a_message_for_the_human(&mut control);
    let mut program = daemon.api();
    let asked = program.subscribe(&[api::Topic::Messages, api::Topic::Attention]);
    let snapshot = asked.snapshot();
    let before = group(snapshot, "g").human_place;
    let unread = &snapshot.attention.as_ref().expect("attention").unread;
    assert_eq!(unread.iter().map(|unread| unread.place).collect::<Vec<_>>(), [before]);

    post(&mut control, &named("a"), said("g", "and another"));
    let read = Asked::Read(msg_request::Read { group: Some("g".to_string()) });
    expect(&mut control, msg(&named("@human"), read), DONE);
    loop {
        let unread = next(&mut program, "the human's unread state", |event| match event {
            api::event::Event::Unread(unread) => Some(unread),
            _ => None,
        });
        assert!(
            unread.waiting.iter().all(|post| unread.place.is_some_and(|place| post.number > place)),
            "{unread:?}"
        );
        if unread.count == 0 {
            assert!(unread.place > before, "{unread:?}");
            break;
        }
    }
}

/// Each member's place moves when it reads, to the newest entry, and never because it posted. A
/// place reaches only entries the client has been told of.
#[test]
fn each_members_place_moves_as_it_reads_and_not_as_it_posts() {
    let daemon = daemon();
    let mut control = daemon.connect();
    a_message_for_the_human(&mut control);
    let mut program = daemon.api();
    let asked = program.subscribe(&[api::Topic::Messages, api::Topic::Places]);
    let (g, places) = (group(asked.snapshot(), "g"), places_of(asked.snapshot(), "g"));
    assert_eq!(places.keys().collect::<Vec<_>>(), ["@human", "a"], "{places:?}");
    assert_eq!(Some(places["@human"]), g.human_place, "{places:?}");
    assert!(places["a"] < g.latest, "a has not read its own post: {places:?}");
    let a_before = places["a"];

    let for_a = msg_request::Post { to: vec!["a".to_string()], ..said("g", "over to you") };
    post(&mut control, &the_human(), for_a);
    let asked = next_logged(&mut program).number;
    read(&mut control, &named("a"));
    let places = next_places(&mut program);
    assert_eq!(places["a"], asked, "{places:?}");
    assert!(places["@human"] < asked, "the human's own post moves their place: {places:?}");

    post(&mut control, &named("a"), said("g", "done"));
    let answered = next_logged(&mut program).number;
    read(&mut control, &the_human());
    let places = next_places(&mut program);
    let expected = BTreeMap::from([("@human".to_string(), answered), ("a".to_string(), asked)]);
    assert_eq!(places, expected);
    assert!(a_before < asked);
}

/// Joining starts a member's place at its Joined entry, past whatever came before it, so a
/// member removed and added back has passed a post it never read: a client counts a place only
/// for posts after the member last joined.
#[test]
fn a_member_added_back_has_its_place_from_its_return() {
    let daemon = daemon();
    let mut control = daemon.connect();
    a_message_for_the_human(&mut control);
    let mut program = daemon.api();
    program.subscribe(&[api::Topic::Messages, api::Topic::Places]);
    let for_a = msg_request::Post { to: vec!["a".to_string()], ..said("g", "read this") };
    post(&mut control, &the_human(), for_a);
    let unread = next_logged(&mut program).number;

    let members = |add: &[&str], remove: &[&str]| {
        let names = |names: &[&str]| names.iter().map(ToString::to_string).collect();
        let change = msg_request::GroupMembers {
            group: "g".to_string(),
            add: names(add),
            remove: names(remove),
        };
        msg(&the_human(), Asked::GroupMembers(change))
    };
    expect(&mut control, members(&[], &["a"]), DONE);
    let places = next_places(&mut program);
    assert!(!places.contains_key("a"), "{places:?}");
    expect(&mut control, members(&["a"], &[]), DONE);
    let returned = next(&mut program, "a joining again", |event| match event {
        api::event::Event::Logged(api::Logged { entry: Some(entry), .. }) => match entry.what {
            Some(api::entry::What::Joined(joined)) if joined.who == "a" => Some(entry.number),
            _ => None,
        },
        _ => None,
    });
    let places = next_places(&mut program);
    assert_eq!(places["a"], returned, "{places:?}");
    assert!(returned > unread);
}

/// A deleted group's places go with it, told to a subscription that hears places alone.
#[test]
fn a_deleted_group_leaves_no_places() {
    let daemon = daemon();
    let mut control = daemon.connect();
    a_message_for_the_human(&mut control);
    let mut program = daemon.api();
    let asked = program.subscribe(&[api::Topic::Places]);
    assert!(asked.snapshot().groups.is_empty(), "{:?}", asked.snapshot());
    assert!(!places_of(asked.snapshot(), "g").is_empty());

    let delete = Asked::GroupDelete(msg_request::GroupDelete { group: "g".to_string() });
    expect(&mut control, msg(&named("a"), delete), DONE);
    assert_eq!(next_places(&mut program), BTreeMap::new());
}

/// A group no member on this machine is in any more has no places: an event with none takes it
/// away, and a fresh snapshot leaves it out.
#[test]
fn a_group_its_last_member_here_left_has_no_places() {
    let daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &named("a"), "a", "g");
    let mut program = daemon.api();
    let asked = program.subscribe(&[api::Topic::Places]);
    assert!(!places_of(asked.snapshot(), "g").is_empty());

    let leave = Asked::Leave(msg_request::Leave { group: Some("g".to_string()) });
    expect(&mut control, msg(&named("a"), leave), DONE);
    assert_eq!(next_places(&mut program), BTreeMap::new());
    let asked = daemon.api().subscribe(&[api::Topic::Places]);
    assert!(asked.snapshot().places.is_empty(), "{:?}", asked.snapshot().places);
}

/// Subscribing and fetching history read nothing for the human and move nobody's place.
#[test]
fn history_and_subscribing_move_nobodys_place() {
    let daemon = daemon();
    let mut control = daemon.connect();
    a_message_for_the_human(&mut control);
    let mut program = daemon.api();
    let topics = [api::Topic::Messages, api::Topic::Places];
    let latest = group(program.subscribe(&topics).snapshot(), "g").latest;
    let asked = program.subscribe(&topics);
    let before = (group(asked.snapshot(), "g").human_place, places_of(asked.snapshot(), "g"));
    fetch(&mut program, "g", 0, latest);

    let asked = daemon.api().subscribe(&topics);
    let after = (group(asked.snapshot(), "g").human_place, places_of(asked.snapshot(), "g"));
    assert_eq!(after, before);
    let notices = snapshot(&mut control).human;
    assert!(notices.iter().any(|notice| notice.group == "g" && notice.count == 1), "{notices:?}");
}

/// History of a deleted group says so, and a group made again under its name says which numbers
/// were the deleted group's.
#[test]
fn a_deleted_group_says_so_and_one_made_again_says_what_it_does_not_hold() {
    let daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &named("a"), "a", "g");
    post(&mut control, &named("a"), said("g", "first life"));
    let mut program = daemon.api();
    let latest = group(program.subscribe(&[api::Topic::Messages]).snapshot(), "g").latest;
    let delete = Asked::GroupDelete(msg_request::GroupDelete { group: "g".to_string() });
    expect(&mut control, msg(&named("a"), delete), DONE);

    let gone = program.ask(history("g", 0, 0));
    assert_eq!(gone.outcome(), api::Outcome::Done);
    assert!(gone.page().deleted && gone.page().entries.is_empty(), "{:?}", gone.page());

    join(&mut control, &named("a"), "a", "g");
    let page = program.ask(history("g", 0, 0));
    let page = page.page();
    assert_eq!((page.deleted, page.held_after), (false, latest));
    assert!(page.entries.iter().all(|entry| entry.number > latest), "{page:?}");

    let unknown = program.ask(history("nowhere", 0, 0));
    assert_eq!(unknown.outcome(), api::Outcome::Refused);
}

/// A question in a page says which post answered it.
#[test]
fn a_question_in_a_page_says_which_post_answered_it() {
    let daemon = daemon();
    let mut control = daemon.connect();
    a_message_for_the_human(&mut control);
    let question = msg_request::Post {
        to: vec!["@human".to_string()],
        answers: vec!["Yes".to_string(), "No".to_string()],
        ..said("g", "Ship it?")
    };
    post(&mut control, &named("a"), question);
    let mut program = daemon.api();
    let latest = group(program.subscribe(&[api::Topic::Messages]).snapshot(), "g").latest;
    let reply = msg_request::Post {
        to: vec!["a".to_string()],
        answering: Some(msg_request::Answering { question: latest, answer: "Yes".to_string() }),
        ..said("g", "Yes")
    };
    post(&mut control, &the_human(), reply);

    let (entries, _) = fetch(&mut program, "g", 0, 0);
    let answered_by = entries.iter().find_map(|entry| match &entry.what {
        Some(api::entry::What::Post(post)) if entry.number == latest => Some(post.answered_by),
        _ => None,
    });
    assert_eq!(answered_by, Some(Some(latest + 1)));
}

/// A post to the human that makes its own group arrives as a Logged event with its body, after
/// the group's own entries, like a post to a group that was already there.
#[test]
fn a_post_that_makes_its_group_arrives_with_its_body() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let joined =
        Asked::Join(msg_request::Join { name: Some("a".to_string()), group: None, pull: false });
    expect(&mut control, msg(&named("a"), joined), DONE);
    let mut program = daemon.api();
    program.subscribe(&[api::Topic::Messages]);

    let to_the_human = msg_request::Post {
        to: vec!["@human".to_string()],
        body: "need input".to_string(),
        ..Default::default()
    };
    post(&mut control, &named("a"), to_the_human);
    let made = next(&mut program, "the group the post made", |event| match event {
        api::event::Event::GroupMade(made) => made.group,
        _ => None,
    });
    let mut entries = Vec::new();
    while entries.last().and_then(body).is_none() {
        entries.push(next_logged(&mut program));
    }
    let expected: Vec<u64> = (made.latest + 1..made.latest + 1 + entries.len() as u64).collect();
    assert_eq!(numbers(&entries), expected, "every entry from the group's first, in order");
    assert_eq!(entries.last().and_then(body), Some("need input"));
}

/// A members request refused partway, after adding one member and before removing a name it
/// cannot find, still tells a program of the member it added.
#[test]
fn a_change_refused_partway_still_tells_a_program_what_it_changed() {
    let daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &named("a"), "a", "g");
    join(&mut control, &named("b"), "b", "elsewhere");
    let mut program = daemon.api();
    program.subscribe(&[api::Topic::Messages]);

    let members = Asked::GroupMembers(msg_request::GroupMembers {
        group: "g".to_string(),
        add: vec!["b".to_string()],
        remove: vec!["nobody".to_string()],
    });
    let refused = control.ask(msg(&named("a"), members));
    assert_eq!(refused.outcome(), proto::Outcome::Refused, "{:?}", refused.answer);
    let changed = next(&mut program, "the member added", |event| match event {
        api::event::Event::GroupChanged(changed) => changed.group,
        _ => None,
    });
    assert_eq!(changed.members, ["a", "b"]);
}
