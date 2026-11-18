use crate::persistence::{stop, until_restored};
use crate::support::*;
use api::{Outcome, Refusal, answer::Detail, event::Event, pane_request::Request};
use proto::pane_request as internal;

fn document(title: &str, body: &str) -> api::CanvasDocument {
    api::CanvasDocument {
        title: title.into(),
        body: body.as_bytes().to_vec(),
        kind: api::CanvasKind::Markdown.into(),
        ..Default::default()
    }
}

fn new_tab() -> api::new_canvas::Placement {
    api::new_canvas::Placement::NewTab(api::new_canvas::NewTab {})
}

fn beside(pane: &str, side: i32, ratio: Option<f32>) -> api::new_canvas::Placement {
    api::new_canvas::Placement::Beside(api::new_canvas::Beside { pane: pane.into(), side, ratio })
}

fn opening(id: &str) -> api::NewCanvas {
    api::NewCanvas {
        operation: id.into(),
        placement: Some(new_tab()),
        document: Some(document("Plan", "original")),
        ..Default::default()
    }
}

fn new_canvas(asked: api::NewCanvas) -> api::request::Request {
    on_pane(Request::NewCanvas(asked))
}

fn write_canvas(pane: &str, doc: Option<api::CanvasDocument>) -> api::request::Request {
    on_pane(Request::WriteCanvas(api::WriteCanvas { pane: pane.into(), document: doc }))
}

fn read_canvas(pane: &str) -> api::request::Request {
    on_pane(Request::ReadCanvas(api::ReadCanvas { pane: pane.into() }))
}

fn made(answer: &api::Answer, outcome: Outcome) -> &api::MadePane {
    assert_eq!(answer.outcome(), outcome, "{answer:?}");
    let Some(Detail::Made(made)) = &answer.detail else {
        panic!("expected the pane id and callsign: {answer:?}");
    };
    made
}

fn read(client: &mut Api, pane: &str) -> api::CanvasRead {
    let read = client.ask(read_canvas(pane));
    assert_eq!(read.outcome(), Outcome::Done, "{:?}", read.answer);
    let Some(Detail::CanvasRead(read)) = read.answer.detail else {
        panic!("expected the document: {:?}", read.answer);
    };
    read
}

fn refusal(asked: ApiAsked, code: Refusal) {
    let answer = asked.answer;
    assert_eq!(answer.outcome(), Outcome::Refused, "{answer:?}");
    assert_eq!(answer.refusal(), code, "{answer:?}");
}

fn changed(event: &api::Event, pane: &str, revision: u64) -> bool {
    matches!(event.event.as_ref(), Some(Event::PaneChanged(change))
        if change.pane.as_ref().is_some_and(|record| record.pane == pane
            && record.canvas.as_ref().is_some_and(|canvas| canvas.revision == revision)))
}

#[test]
fn the_api_opens_a_canvas_and_both_protocols_see_its_edits() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let mut author = daemon.api();
    let mut reader = daemon.api();
    author.subscribe(&[api::Topic::Panes]);
    reader.subscribe(&[api::Topic::Panes]);

    let opened = author.ask(new_canvas(api::NewCanvas {
        name: Some("Notes".into()),
        opened_by: Some("p1".into()),
        placement: Some(beside("p1", api::new_canvas::Side::Right.into(), Some(0.35))),
        ..opening("split")
    }));
    let made = made(&opened.answer, Outcome::Done);
    let pane_id = made.pane.as_str();
    assert_eq!(tab_shape(&mut control, "t1"), format!("[p1|{pane_id} 0.35]"));
    let (event, appeared) = opened
        .events
        .iter()
        .find_map(|event| match &event.event {
            Some(Event::PaneAppeared(appeared)) => Some((event, appeared.pane.as_ref().unwrap())),
            _ => None,
        })
        .unwrap();
    assert_eq!(event.correlation, opened.answer.correlation);
    assert_eq!(appeared.pane, pane_id);
    assert_eq!(appeared.name, "Notes");
    assert_eq!(appeared.callsign, made.callsign);
    assert_eq!(
        appeared.canvas,
        Some(api::Canvas {
            revision: 1,
            title: "Plan".into(),
            kind: api::CanvasKind::Markdown.into(),
            bytes: 8,
            opened_by: Some("p1".into()),
        })
    );

    let updated = document("Revised", "new text");
    let written = author.ask(write_canvas(pane_id, Some(updated.clone())));
    assert_eq!(written.outcome(), Outcome::Done);
    assert_eq!(
        written.answer.detail,
        Some(Detail::CanvasWritten(api::CanvasWritten { revision: 2 }))
    );
    let event = written.events.iter().find(|event| changed(event, pane_id, 2)).unwrap();
    assert_eq!(event.correlation, written.answer.correlation);
    let other =
        reader.event_matching("the new canvas revision", |event| changed(event, pane_id, 2));
    assert!(other.correlation.is_empty());
    let from_control = expect(
        &mut control,
        pane(internal::Request::ReadCanvas(internal::ReadCanvas { pane: pane_id.into() })),
        proto::Outcome::Done,
    );
    let Some(proto::answer::Detail::CanvasRead(from_control)) = from_control.answer.detail else {
        panic!("expected a control canvas read");
    };
    assert_eq!(from_control.revision, 2);
    assert_eq!(from_control.document.unwrap().body, updated.body);

    let html = proto::CanvasDocument {
        kind: proto::CanvasKind::Html.into(),
        title: "HTML".into(),
        body: b"<b>third</b>".to_vec(),
        ..Default::default()
    };
    expect(
        &mut control,
        pane(internal::Request::WriteCanvas(internal::WriteCanvas {
            pane: pane_id.into(),
            document: Some(html.clone()),
        })),
        proto::Outcome::Done,
    );
    let back = read(&mut author, pane_id);
    let back_doc = back.document.unwrap();
    assert_eq!(back.revision, 3);
    assert_eq!(back_doc.kind(), api::CanvasKind::Html);
    assert_eq!(back_doc.body, html.body);
    assert_eq!(back_doc.title, html.title);
}

#[test]
fn a_new_canvas_uses_the_same_operation_id_across_reconnect_and_restart() {
    let mut daemon = daemon();
    let mut client = daemon.api();
    let request = opening("make-canvas");
    let first = made(&client.ask(new_canvas(request.clone())).answer, Outcome::Done).clone();
    let mut control = daemon.connect();
    let before = snapshot(&mut control);
    assert_eq!(before.tabs.len(), 1);
    assert_eq!(shape(before.tabs[0].root.as_ref().unwrap()), first.pane);
    assert!(!before.tabs[0].hidden);
    drop(client);

    let mut client = daemon.api();
    assert_eq!(made(&client.ask(new_canvas(request.clone())).answer, Outcome::AlreadySo), &first);
    refusal(
        client.ask(new_canvas(api::NewCanvas { name: Some("other".into()), ..request.clone() })),
        Refusal::OperationReused,
    );
    refusal(
        client.ask(new_pane(api::NewPane {
            operation: request.operation.clone(),
            ..Default::default()
        })),
        Refusal::OperationReused,
    );

    stop(&mut daemon, &mut control);
    daemon.restart();
    let mut control = daemon.connect();
    let mut client = daemon.api();
    assert_eq!(made(&client.ask(new_canvas(request)).answer, Outcome::AlreadySo), &first);
    assert_eq!(until_restored(&mut control, 1).panes[0].pane, first.pane);
}

#[test]
fn an_in_flight_canvas_operation_is_never_started_twice() {
    let daemon = daemon_with(&[("MUSTER_DAEMON_CANVAS_FAULT", "pause-before-create-rename")]);
    let mut first = daemon.api();
    let request = new_canvas(opening("one operation"));
    let pending = first.send(request.clone());
    until_file(&daemon.socket_path().with_extension("canvas-pause"), "the create to pause");
    let mut other = daemon.api();
    let repeated = other.ask(request.clone());
    assert_eq!(repeated.outcome(), Outcome::Unknown, "{:?}", repeated.answer);
    assert!(repeated.answer.reason.contains("still being carried out"));
    assert!(snapshot(&mut daemon.connect()).panes.is_empty());

    std::fs::write(daemon.socket_path().with_extension("canvas-resume"), []).unwrap();
    let Some(api::daemon_message::Message::Answer(first)) =
        first.next_message(muster_harness::PATIENCE)
    else {
        panic!("expected the paused create to answer");
    };
    assert_eq!(first.correlation, pending);
    let repeated = other.ask(request);
    assert_eq!(made(&repeated.answer, Outcome::AlreadySo), made(&first, Outcome::Done));
    assert_eq!(snapshot(&mut daemon.connect()).panes.len(), 1);
}

#[test]
fn the_api_refuses_bad_canvas_placement_and_documents() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let mut client = daemon.api();
    let base = opening("may-retry");
    for request in [
        api::NewCanvas { operation: String::new(), ..base.clone() },
        api::NewCanvas { placement: None, ..base.clone() },
        api::NewCanvas { opened_by: Some("bad id".into()), ..base.clone() },
        api::NewCanvas { document: None, ..base.clone() },
        api::NewCanvas { placement: Some(beside("p1", 99, None)), ..base.clone() },
        api::NewCanvas {
            placement: Some(beside("p1", api::new_canvas::Side::Left.into(), Some(1.0))),
            ..base.clone()
        },
    ] {
        refusal(client.ask(new_canvas(request)), Refusal::Invalid);
    }
    refusal(
        client.ask(new_canvas(api::NewCanvas {
            placement: Some(beside("gone", api::new_canvas::Side::Left.into(), None)),
            ..base.clone()
        })),
        Refusal::NotFound,
    );
    let opened = client.ask(new_canvas(base));
    let pane_id = &made(&opened.answer, Outcome::Done).pane;
    for unknown in [read_canvas("gone"), write_canvas("gone", Some(document("", "a")))] {
        refusal(client.ask(unknown), Refusal::NotFound);
    }
    for terminal in [read_canvas("p1"), write_canvas("p1", Some(document("", "a")))] {
        refusal(client.ask(terminal), Refusal::Invalid);
    }

    for invalid in [
        None,
        Some(api::CanvasDocument { kind: 99, ..document("", "a") }),
        Some(api::CanvasDocument { body: vec![0xff], ..document("", "a") }),
        Some(document("not\none line", "a")),
        Some(document(&"é".repeat(129), "a")),
        Some(api::CanvasDocument {
            kind: api::CanvasKind::Image.into(),
            image_type: "image/png".into(),
            ..document("", "GIF89a")
        }),
    ] {
        refusal(client.ask(write_canvas(pane_id, invalid)), Refusal::Invalid);
    }
    assert_eq!(read(&mut client, pane_id).revision, 1);
}

#[test]
fn the_api_round_trips_a_body_at_the_canvas_limit() {
    let daemon = daemon();
    let mut client = daemon.api();
    let mut large = api::CanvasDocument { body: vec![b'a'; 4 << 20], ..document("Max", "") };
    let created = client
        .ask(new_canvas(api::NewCanvas { document: Some(large.clone()), ..opening("large") }));
    let pane_id = &made(&created.answer, Outcome::Done).pane;
    let read_back = read(&mut client, pane_id);
    assert_eq!(read_back, api::CanvasRead { revision: 1, document: Some(large.clone()) });

    large.body.push(b'a');
    refusal(client.ask(write_canvas(pane_id, Some(large))), Refusal::Invalid);
    assert_eq!(read(&mut client, pane_id), read_back);
}

#[test]
fn the_api_distinguishes_a_refused_write_from_an_unconfirmed_rename() {
    for (fault, outcome, revision) in
        [("fail-before-rename", Outcome::Refused, 1), ("fail-after-rename", Outcome::Unknown, 2)]
    {
        let daemon = daemon_with(&[("MUSTER_DAEMON_CANVAS_FAULT", fault)]);
        let mut client = daemon.api();
        let created = client.ask(new_canvas(opening("make")));
        let pane_id = &made(&created.answer, Outcome::Done).pane;
        client.subscribe(&[api::Topic::Panes]);
        let asked = client.ask(write_canvas(pane_id, Some(document("Next", "new"))));
        assert_eq!(asked.outcome(), outcome, "{:?}", asked.answer);
        if outcome == Outcome::Unknown {
            assert!(asked.answer.reason.contains("durability"));
            let event = asked.events.iter().find(|event| changed(event, pane_id, 2)).unwrap();
            assert_eq!(event.correlation, asked.answer.correlation);
        } else {
            assert!(asked.events.is_empty(), "{:?}", asked.events);
        }
        let back = read(&mut client, pane_id);
        assert_eq!(back.revision, revision);
        let expected =
            if revision == 1 { document("Plan", "original") } else { document("Next", "new") };
        assert_eq!(back.document, Some(expected));
    }
}

#[test]
fn canvas_requests_are_unavailable_while_the_daemon_hands_over() {
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-after-accept")]);
    let mut client = daemon.api();
    let created = client.ask(new_canvas(opening("make")));
    let original = made(&created.answer, Outcome::Done);
    let pane_id = &original.pane;
    let replacing = daemon.start_replacing(None);
    daemon.paused();

    for request in [
        read_canvas(pane_id),
        write_canvas(pane_id, Some(document("", "new"))),
        new_canvas(opening("second")),
    ] {
        refusal(client.ask(request), Refusal::Unavailable);
    }

    daemon.resume();
    let replaced = daemon.finish_replacing(replacing);
    assert_eq!(replaced.outcome(), proto::Outcome::Done, "{replaced:?}");
    let mut successor = daemon.api();
    assert_eq!(read(&mut successor, pane_id).revision, 1);
    let repeated = successor.ask(new_canvas(opening("make")));
    assert_eq!(made(&repeated.answer, Outcome::AlreadySo), original);
}

#[test]
fn the_api_refuses_when_the_canvas_file_cannot_be_read() {
    let daemon = daemon();
    let mut client = daemon.api();
    let created = client.ask(new_canvas(opening("make")));
    let pane_id = &made(&created.answer, Outcome::Done).pane;
    let file = daemon.root().join("daemon.sock.canvases").join(pane_id);
    std::fs::rename(file, daemon.root().join("unavailable-doc")).unwrap();

    let refused = client.ask(read_canvas(pane_id));
    assert!(refused.answer.reason.contains("could not read canvas"), "{:?}", refused.answer);
    refusal(refused, Refusal::Unspecified);
}
