//! The API for other programs (MIP-10): its handshake, a subscription's snapshot and numbering,
//! and how a connection ends.

use std::time::Duration;

use crate::persistence::{directory, restarted_from, saved, stop};
use crate::support::*;
use muster_api::connection;
use muster_api::version::API;
use prost::Message;

fn every_topic() -> [api::Topic; 4] {
    [api::Topic::Panes, api::Topic::Machines, api::Topic::Messages, api::Topic::Attention]
}

fn source_of(welcome: &api::Welcome) -> api::Source {
    welcome.machine.as_ref().and_then(|machine| machine.source.clone()).expect("a source")
}

fn identity_of(daemon: &Daemon) -> api::Source {
    source_of(daemon.api().welcome())
}

/// The Hello carries an internal protocol of another major, and the daemon never judges it by
/// that: an API connection is told apart by its kind alone.
#[test]
fn an_api_client_is_welcomed_by_its_own_version_whatever_the_internal_one_says() {
    let daemon = daemon();
    let internal = proto::Hello {
        protocol: Some(proto::Version { major: 99, minor: 0 }),
        kind: proto::ConnectionKind::Api.into(),
        client: "from the future".to_string(),
    };
    let mut hello = internal.encode_to_vec();
    hello.extend(connection::hello(test_client()).encode_to_vec());
    let api = Api::try_connect_with_hello(daemon.socket_path(), &hello)
        .unwrap_or_else(|error| panic!("an API client was not welcomed: {error}"));

    let welcome = api.welcome();
    assert_eq!(welcome.api, Some(API));
    let machine = welcome.machine.as_ref().expect("the machine reached");
    let source = machine.source.as_ref().expect("its source");
    assert!(source.daemon.starts_with('d') && source.daemon.len() > 1, "{source:?}");
    assert_eq!(source.instance, daemon.connect().welcome().instance);
    assert!(!machine.name.is_empty());
    assert!(!machine.version.is_empty());
    let features = machine.features.as_ref().expect("its features");
    assert_eq!(features.requests, ["api.subscribe"]);
    let topics: Vec<i32> = every_topic().iter().map(|topic| (*topic).into()).collect();
    assert_eq!(features.topics, topics);
}

/// The kind is read before either schema's other fields: an API Hello stays readable whatever
/// the internal Hello would make of its other fields.
#[test]
fn an_api_hello_is_read_whatever_the_internal_hello_would_make_of_it() {
    let daemon = daemon();
    // Field 1 as a number, where the internal Hello holds its version as a message.
    let mut hello = vec![0x08, 0x07];
    hello.extend(connection::hello(test_client()).encode_to_vec());
    assert!(proto::Hello::decode(hello.as_slice()).is_err(), "the internal Hello reads it");
    let api = Api::try_connect_with_hello(daemon.socket_path(), &hello)
        .unwrap_or_else(|error| panic!("an API client was not welcomed: {error}"));
    assert_eq!(api.welcome().api, Some(API));
}

#[test]
fn a_client_of_another_api_major_is_refused_with_the_daemons_version_and_hung_up_on() {
    let daemon = daemon();
    let mut hello = connection::hello(test_client());
    hello.api = Some(api::Version { major: API.major + 1, minor: 0 });
    let refused = match Api::try_connect_with_hello(daemon.socket_path(), &hello.encode_to_vec()) {
        Err(connection::HandshakeError::Refused(refused)) => refused,
        other => panic!("a client of another major was not refused: {other:?}"),
    };
    assert_eq!(refused.api, Some(API));
    assert!(refused.reason.contains("major"), "{}", refused.reason);
    // And everyone else is still served.
    daemon.connect();
    daemon.api();
}

/// A daemon older than the API answers an API Hello with its own refusal, which a client reads
/// as one, naming no API version.
#[test]
fn a_daemons_internal_refusal_reads_as_an_api_refusal() {
    let internal = proto::HelloAnswer {
        answer: Some(proto::hello_answer::Answer::Refused(proto::HelloRefused {
            reason: "the hello names no protocol version".to_string(),
            daemon: Some(proto::Version { major: 1, minor: 8 }),
        })),
    };
    let read = api::HelloAnswer::decode(internal.encode_to_vec().as_slice()).unwrap();
    let Some(api::hello_answer::Answer::Refused(refused)) = read.answer else {
        panic!("an internal refusal read as {read:?}");
    };
    assert_eq!(refused.reason, "the hello names no protocol version");
    assert_eq!(refused.api, None);
}

#[test]
fn a_subscription_starts_from_a_snapshot_in_an_epoch_of_its_own() {
    let daemon = daemon();
    let mut api = daemon.api();
    let source = source_of(api.welcome());
    let asked = api.subscribe(&every_topic());
    assert!(asked.events.is_empty(), "nothing from before the subscription");
    let snapshot = asked.snapshot();
    assert_ne!(snapshot.epoch, 0);
    assert_eq!((asked.answer.epoch, asked.answer.through), (snapshot.epoch, 0));
    assert_eq!(snapshot.through, 0);
    let topics: Vec<i32> = every_topic().iter().map(|topic| (*topic).into()).collect();
    let mut asked_for = snapshot.topics.clone();
    asked_for.sort_unstable();
    assert_eq!(asked_for, topics);
    assert_eq!(
        snapshot.sources,
        [api::SourceStatus {
            source: Some(source.clone()),
            state: api::SourceState::Current.into()
        }]
    );
    assert_eq!(snapshot.machines.len(), 1);
    assert_eq!(snapshot.machines[0].source.as_ref(), Some(&source));
    // What proves a negative here is that the next answer arrived first: an event a request
    // caused comes before its answer.
    assert!(api.next_message(Duration::from_millis(50)).is_none());

    let again = api.subscribe(&[]);
    assert_ne!(again.snapshot().epoch, snapshot.epoch, "a new subscription is a new epoch");
}

#[test]
fn a_snapshot_holds_only_the_topics_asked_for_and_always_its_sources() {
    let daemon = daemon();
    let mut api = daemon.api();
    let asked = api.subscribe(&[api::Topic::Panes, api::Topic::Panes]);
    let snapshot = asked.snapshot();
    assert_eq!(snapshot.topics, [i32::from(api::Topic::Panes)], "a topic named twice counts once");
    assert!(snapshot.machines.is_empty());
    assert_eq!(snapshot.sources.len(), 1);
}

#[test]
fn a_topic_or_request_the_daemon_does_not_serve_is_refused_and_the_connection_stays() {
    let daemon = daemon();
    let mut api = daemon.api();
    let unknown = api::request::Request::Api(api::ApiRequest {
        request: Some(api::api_request::Request::Subscribe(api::Subscribe { topics: vec![99] })),
    });
    let asked = api.ask(unknown);
    assert_eq!(asked.outcome(), api::Outcome::Refused);
    assert!(asked.answer.reason.contains("99"), "{}", asked.answer.reason);

    let empty = api.ask(api::request::Request::Api(api::ApiRequest { request: None }));
    assert_eq!(empty.outcome(), api::Outcome::Refused);
    assert!(empty.answer.correlation.starts_with("harness-"), "{:?}", empty.answer);
    assert_eq!((empty.answer.epoch, empty.answer.through), (0, 0), "nothing was subscribed");

    api.subscribe(&[]);
}

/// A daemon bringing back the tabs it saved says so as a source, and that it is current once it
/// has: the first event in the subscription.
#[test]
fn a_restoring_daemon_says_so_and_then_that_it_is_current() {
    // As many tabs as the persistence suite takes to outlast one answer.
    const TABS: usize = 20;
    let mut daemon = daemon();
    let work = directory(&daemon, "work");
    let _control = restarted_from(&mut daemon, &saved(TABS, &work, None));
    let mut api = daemon.api();
    let source = source_of(api.welcome());
    let asked = api.subscribe(&[]);
    let snapshot = asked.snapshot();
    assert_eq!(
        snapshot.sources,
        [api::SourceStatus {
            source: Some(source.clone()),
            state: api::SourceState::Restoring.into()
        }],
        "{TABS} tabs take longer than one answer to bring back"
    );

    let event = api.next_event();
    assert_eq!((event.epoch, event.number), (snapshot.epoch, 1));
    assert_eq!(event.source.as_ref(), Some(&source));
    let current =
        api::SourceStatus { source: Some(source), state: api::SourceState::Current.into() };
    assert_eq!(
        event.event,
        Some(api::event::Event::SourceChanged(api::SourceChanged { status: Some(current) }))
    );
}

/// A daemon's identity is drawn once, beside its socket, and kept by every later run and every
/// successor; its instance is new each time.
#[test]
fn a_daemon_keeps_its_identity_across_a_restart_and_a_handoff_with_a_new_instance_each_time() {
    let mut daemon = daemon();
    let first = identity_of(&daemon);
    let written = std::fs::read_to_string(daemon.root().join("daemon.identity")).unwrap();
    assert_eq!(written.trim(), first.daemon);

    let mut control = daemon.connect();
    stop(&mut daemon, &mut control);
    daemon.restart();
    let restarted = identity_of(&daemon);
    assert_eq!(restarted.daemon, first.daemon);
    assert_ne!(restarted.instance, first.instance);

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    let successor = identity_of(&daemon);
    assert_eq!(successor.daemon, first.daemon);
    assert_ne!(successor.instance, restarted.instance);
}

/// A handoff ends every API connection, saying so, and the successor starts a subscription in a
/// new epoch.
#[test]
fn a_handoff_ends_every_api_connection_and_the_successor_subscribes_in_a_new_epoch() {
    let mut daemon = daemon();
    let mut subscribed = daemon.api();
    let before = subscribed.subscribe(&every_topic()).snapshot().clone();
    let mut idle = daemon.api();

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    let (ended, _) = subscribed.ended();
    assert_eq!(ended.reason(), api::ended::Reason::Replaced);
    assert_eq!((ended.epoch, ended.reached), (before.epoch, 0));
    let (ended, _) = idle.ended();
    assert_eq!(ended.reason(), api::ended::Reason::Replaced);
    assert_eq!((ended.epoch, ended.reached), (0, 0));

    let mut again = daemon.api();
    let after = again.subscribe(&every_topic()).snapshot().clone();
    assert_ne!(after.epoch, before.epoch);
    let (was, is) = (&before.sources[0], &after.sources[0]);
    assert_eq!(was.source.as_ref().unwrap().daemon, is.source.as_ref().unwrap().daemon);
    assert_ne!(was.source.as_ref().unwrap().instance, is.source.as_ref().unwrap().instance);
}

#[test]
fn a_stopping_daemon_ends_every_api_connection_saying_so() {
    let mut daemon = daemon();
    let mut api = daemon.api();
    let snapshot = api.subscribe(&[]).snapshot().clone();
    let mut control = daemon.connect();
    stop(&mut daemon, &mut control);
    let (ended, _) = api.ended();
    assert_eq!(ended.reason(), api::ended::Reason::Stopping);
    assert_eq!(ended.epoch, snapshot.epoch);
}

/// A client that sends and never reads is dropped once what waits for it passes its byte budget,
/// told the number it reached, and subscribes again on a new connection.
#[test]
fn a_client_that_stops_reading_is_dropped_at_its_byte_budget_and_subscribes_again() {
    let daemon = daemon_with(&[("MUSTER_DAEMON_API_BUDGET", "16384")]);
    let mut log = daemon.connect();
    let followed = log.ask(session(proto::session_request::Request::FollowLog(
        proto::session_request::FollowLog { after: None },
    )));
    assert_eq!(followed.outcome(), proto::Outcome::Done);

    let mut api = daemon.api();
    let mut writer = api.writer();
    writer.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
    // Far more answers than the budget and the kernel's buffers hold together, on any system.
    let sending = std::thread::spawn(move || {
        let request =
            api::Request { correlation: "flood".to_string(), request: Some(subscribe_to(&[])) };
        for _ in 0..50_000 {
            if connection::send(&mut writer, &request).is_err() {
                return;
            }
        }
    });
    let logged = log.logged_until("daemon.connection.overrun", muster_harness::PATIENCE);
    assert!(
        logged.iter().any(|line| line.line.contains("daemon.connection.overrun")),
        "the client never fell behind"
    );

    let (ended, before) = api.ended();
    assert_eq!(ended.reason(), api::ended::Reason::FellBehind);
    // Nothing at all is written when the budget ran out before the daemon's writer started.
    let reached = before.iter().fold((0, 0), |_, message| match message {
        api::daemon_message::Message::Answer(answer) => (answer.epoch, answer.through),
        other => panic!("only answers were asked for, and the daemon sent {other:?}"),
    });
    assert_eq!((ended.epoch, ended.reached), reached);
    assert!(ended.detail.contains("16384 bytes"), "{}", ended.detail);
    sending.join().unwrap();

    daemon.api().subscribe(&every_topic());
}
