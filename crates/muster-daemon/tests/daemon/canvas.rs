use std::path::PathBuf;

use crate::persistence::record;
use crate::support::*;
use proto::answer::Detail;
use proto::event::Event;
use proto::{Outcome, pane_request as p, session_request};

const FAULT: &str = "MUSTER_DAEMON_CANVAS_FAULT";

fn markdown(title: &str, text: &str) -> proto::CanvasDocument {
    proto::CanvasDocument {
        kind: proto::CanvasKind::Markdown.into(),
        title: title.into(),
        body: text.as_bytes().to_vec(),
        ..Default::default()
    }
}

fn opening(name: &str, placement: proto::Placement, document: proto::CanvasDocument) -> p::Request {
    p::Request::CreateCanvas(p::CreateCanvas {
        pane: name.into(),
        placement: Some(placement),
        label: Some("Notes".into()),
        document: Some(document),
        quiet: true,
        opened_by: Some("p1".into()),
        ..Default::default()
    })
}

fn open(control: &mut Control, name: &str, text: &str) {
    expect(
        control,
        pane(opening(name, in_new_tab("notes"), markdown("Plan", text))),
        Outcome::Done,
    );
}

fn writing(name: &str, document: proto::CanvasDocument) -> proto::request::Service {
    pane(p::Request::WriteCanvas(p::WriteCanvas { pane: name.into(), document: Some(document) }))
}

fn reading(name: &str) -> proto::request::Service {
    pane(p::Request::ReadCanvas(p::ReadCanvas { pane: name.into() }))
}

fn read(control: &mut Control, name: &str) -> proto::CanvasRead {
    let asked = expect(control, reading(name), Outcome::Done);
    let Some(Detail::CanvasRead(read)) = asked.answer.detail else {
        panic!("expected a canvas read: {:?}", asked.answer);
    };
    read
}

fn revision(answer: &proto::Answer) -> u64 {
    assert_eq!(answer.outcome(), Outcome::Done, "{}", answer.reason);
    let Some(Detail::CanvasWritten(written)) = &answer.detail else {
        panic!("expected a written revision: {answer:?}");
    };
    written.revision
}

fn file(daemon: &Daemon, name: &str) -> PathBuf {
    let mut directory = daemon.socket_path().as_os_str().to_os_string();
    directory.push(".canvases");
    PathBuf::from(directory).join(name)
}

#[test]
fn a_canvas_is_announced_before_its_split_and_writes_publish_the_revision() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    expect(&mut control, subscribe_request(), Outcome::Done);
    let first = markdown("Plan", "First");
    let opened = expect(
        &mut control,
        pane(opening("paper", beside("p1", proto::Side::Right), first.clone())),
        Outcome::Done,
    );
    let Some(Detail::Created(created)) = opened.answer.detail else {
        panic!("expected a callsign");
    };
    let events: Vec<_> = opened.events.iter().filter_map(|event| event.event.as_ref()).collect();
    let [Event::PaneOpened(open), Event::TabChanged(_)] = events.as_slice() else {
        panic!("pane must open before its tab names it: {events:?}");
    };
    let pane_record = open.pane.as_ref().unwrap();
    let canvas = pane_record.canvas.as_ref().unwrap();
    assert_eq!(pane_record.callsign.as_deref(), Some(created.callsign.as_str()));
    assert!(pane_record.quiet);
    assert!(pane_record.maker.is_none());
    assert!(pane_record.cwd.is_empty());
    assert_eq!(canvas.opened_by.as_deref(), Some("p1"));
    assert_eq!((canvas.revision, canvas.title.as_str(), canvas.bytes), (1, "Plan", 5));
    assert_eq!(
        read(&mut control, "paper"),
        proto::CanvasRead { revision: 1, document: Some(first) }
    );
    let next = markdown("Revised", "Next");
    let written = expect(&mut control, writing("paper", next.clone()), Outcome::Done);
    assert_eq!(revision(&written.answer), 2);
    assert!(written.events.iter().any(|event| {
        matches!(event.event.as_ref(), Some(Event::PaneChanged(changed))
            if changed.pane.as_ref().and_then(|pane| pane.canvas.as_ref()).is_some_and(|canvas|
                canvas.revision == 2 && canvas.bytes == 4 && canvas.title == "Revised"))
    }));
    assert_eq!(
        read(&mut control, "paper"),
        proto::CanvasRead { revision: 2, document: Some(next) }
    );
}

#[test]
fn a_duplicate_create_and_refused_write_keep_the_original_document() {
    let daemon = daemon();
    let mut control = daemon.connect();
    open(&mut control, "paper", "old");
    let duplicate = pane(opening("paper", in_new_tab("notes"), markdown("duplicate", "new")));
    expect(&mut control, duplicate, Outcome::AlreadySo);
    make(&mut control, create("p1", in_new_tab("t1")));
    for request in [reading("gone"), writing("gone", markdown("", "no"))] {
        expect(&mut control, request, Outcome::NotThere);
    }
    for request in [reading("p1"), writing("p1", markdown("", "no"))] {
        let asked = expect(&mut control, request, Outcome::Refused);
        assert!(asked.answer.reason.contains("canvas"));
    }
    let refused = expect(&mut control, read_request("paper", 0, 1), Outcome::Refused);
    assert!(refused.answer.reason.contains("muster canvas read"));
    let mut invalid = markdown("", "invalid");
    invalid.body.push(0xff);
    expect(&mut control, writing("paper", invalid), Outcome::Refused);
    for title in ["é".repeat(129), "first\nsecond".into()] {
        let refusal =
            expect(&mut control, writing("paper", markdown(&title, "new")), Outcome::Refused);
        assert!(refusal.answer.reason.contains("title"));
    }
    let mut image = markdown("", "GIF89a\u{1}\0\u{1}\0\0\0\0;");
    image.set_kind(proto::CanvasKind::Image);
    image.image_type = "image/gif".into();
    expect(&mut control, writing("paper", image), Outcome::Refused);
    let original = read(&mut control, "paper");
    assert_eq!(original.revision, 1);
    assert_eq!(original.document.unwrap().body, b"old");
}

#[test]
fn a_canvas_does_not_keep_its_openers_lifetime_or_accept_terminal_verbs() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    open(&mut control, "paper", "notes");
    expect(&mut control, close_request("p1"), Outcome::Done);
    for request in [
        p::Request::Clear(p::Clear { pane: "paper".into(), ..Default::default() }),
        p::Request::Compact(p::Compact { pane: "paper".into(), ..Default::default() }),
        p::Request::Restart(p::Restart { pane: "paper".into(), ..Default::default() }),
        p::Request::Report(p::Report { pane: "paper".into(), ..Default::default() }),
        p::Request::StoreImage(p::StoreImage { pane: "paper".into(), png: Vec::new() }),
        p::Request::Send(p::Send { pane: "paper".into(), text: "no".into(), ..Default::default() }),
    ] {
        let refusal = expect(&mut control, pane(request), Outcome::Refused);
        assert!(refusal.answer.reason.contains("canvas"), "{:?}", refusal.answer);
    }
    assert_eq!(read(&mut control, "paper").document.unwrap().body, b"notes");
}

#[test]
fn a_write_that_fails_before_rename_keeps_the_previous_document_and_revision() {
    let daemon = daemon_with(&[(FAULT, "fail-before-rename")]);
    let mut control = daemon.connect();
    open(&mut control, "paper", "old");
    expect(&mut control, subscribe_request(), Outcome::Done);
    let failed = expect(&mut control, writing("paper", markdown("", "new")), Outcome::Refused);
    assert!(failed.answer.reason.contains("before rename"));
    assert!(failed.events.is_empty(), "{:?}", failed.events);
    assert_eq!(read(&mut control, "paper").document.unwrap().body, b"old");
    assert_eq!(record(&snapshot(&mut control), "paper").canvas.as_ref().unwrap().revision, 1);
}

#[test]
fn a_write_with_unknown_durability_publishes_the_revision_it_reads_back() {
    let daemon = daemon_with(&[(FAULT, "fail-after-rename")]);
    let mut control = daemon.connect();
    open(&mut control, "paper", "old");
    expect(&mut control, subscribe_request(), Outcome::Done);
    let failed =
        expect(&mut control, writing("paper", markdown("second", "new")), Outcome::Refused);
    assert!(failed.answer.reason.contains("durability of this write is unknown"));
    assert!(failed.events.iter().any(|event| {
        matches!(event.event.as_ref(), Some(Event::PaneChanged(changed))
            if changed.pane.as_ref().and_then(|pane| pane.canvas.as_ref()).is_some_and(|canvas|
                canvas.revision == 2 && canvas.title == "second"))
    }));
    assert_eq!(
        read(&mut control, "paper"),
        proto::CanvasRead { revision: 2, document: Some(markdown("second", "new")) }
    );
}

#[test]
fn a_daemon_interrupted_before_canvas_rename_keeps_the_previous_file() {
    let mut daemon = daemon_with(&[(FAULT, "exit-before-rename")]);
    let mut control = daemon.connect();
    open(&mut control, "paper", "old");
    let path = file(&daemon, "paper");
    let before = std::fs::read(&path).unwrap();
    control.send(writing("paper", markdown("", "new")));
    assert_eq!(daemon.wait_for_exit().code(), Some(97));
    assert_eq!(std::fs::read(path).unwrap(), before);
}

fn wait_for_write(daemon: &Daemon) {
    let path = daemon.socket_path().with_extension("canvas-pause");
    until_file(&path, "the canvas write to pause at rename");
}

fn resume_write(daemon: &Daemon) {
    let path = daemon.socket_path().with_extension("canvas-resume");
    std::fs::write(path, []).unwrap();
}

fn wait_in_log(control: &mut Control, event: &str) {
    let lines = control.logged_until(event, muster_harness::PATIENCE);
    assert!(lines.iter().any(|line| line.line.contains(event)), "{event} never arrived");
}

fn answer(control: &mut Control, id: u64) -> proto::Answer {
    let Some(proto::control_message::Message::Answer(answer)) =
        control.next_message(muster_harness::PATIENCE)
    else {
        panic!("expected answer {id}");
    };
    assert_eq!(answer.id, id);
    answer
}

#[test]
fn overlapping_writers_get_successive_revisions_and_the_last_document_wins() {
    let daemon = daemon_with(&[(FAULT, "pause-before-rename")]);
    let mut first = daemon.connect();
    open(&mut first, "paper", "first");
    let mut logging = following(&daemon);
    let id = first.send(writing("paper", markdown("second", "2")));
    wait_for_write(&daemon);
    let mut second = daemon.connect();
    let other = second.send(writing("paper", markdown("third", "3")));
    wait_in_log(&mut logging, "daemon.canvas.document_wait");
    let mut observer = daemon.connect();
    assert_eq!(record(&snapshot(&mut observer), "paper").canvas.as_ref().unwrap().revision, 1);
    resume_write(&daemon);
    assert_eq!(revision(&answer(&mut first, id)), 2);
    assert_eq!(revision(&answer(&mut second, other)), 3);
    assert_eq!(read(&mut observer, "paper").document.unwrap().body, b"3");
}

#[test]
fn closing_a_canvas_waits_for_its_admitted_write() {
    let daemon = daemon_with(&[(FAULT, "pause-before-rename")]);
    let mut writer = daemon.connect();
    open(&mut writer, "paper", "first");
    let mut observer = daemon.connect();
    expect(&mut observer, subscribe_request(), Outcome::Done);
    let mut logging = following(&daemon);
    let id = writer.send(writing("paper", markdown("second", "2")));
    wait_for_write(&daemon);
    let mut close = daemon.connect();
    let closing = close.send(close_request("paper"));
    wait_in_log(&mut logging, "daemon.canvas.exclusive_wait");
    assert!(snapshot(&mut daemon.connect()).panes.iter().any(|pane| pane.pane == "paper"));
    resume_write(&daemon);
    assert_eq!(revision(&answer(&mut writer, id)), 2);
    assert_eq!(answer(&mut close, closing).outcome(), Outcome::Done);
    let updated = observer.next_event();
    assert!(matches!(updated.event, Some(Event::PaneChanged(_))), "{updated:?}");
    assert!(snapshot(&mut daemon.connect()).panes.is_empty());
}

#[test]
fn stopping_waits_for_the_first_canvas_create_and_its_answer() {
    let mut daemon = daemon_with(&[(FAULT, "pause-before-create-rename")]);
    let mut creator = daemon.connect();
    let mut logging = following(&daemon);
    let opened = creator.send(pane(opening("paper", in_new_tab("notes"), markdown("", "first"))));
    wait_for_write(&daemon);
    let mut stop = daemon.connect();
    let stopped = stop.send(session(session_request::Request::Stop(session_request::Stop {})));
    wait_in_log(&mut logging, "daemon.canvas.exclusive_wait");
    resume_write(&daemon);
    assert_eq!(answer(&mut creator, opened).outcome(), Outcome::Done);
    assert_eq!(answer(&mut stop, stopped).outcome(), Outcome::Done);
    assert!(daemon.wait_for_exit().success());
}
