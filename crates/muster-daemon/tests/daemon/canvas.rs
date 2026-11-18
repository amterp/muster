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

fn saved_state(daemon: &Daemon) -> serde_json::Value {
    serde_json::from_str(&written(&crate::persistence::state_file(daemon))).unwrap()
}

#[test]
fn a_mixed_tab_restores_the_latest_canvas_and_reopens_it_in_place() {
    use crate::persistence::{stop, until_restored, until_saved};
    use crate::reopen::{close, reopened};

    let mut daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    expect(
        &mut control,
        pane(opening("paper", beside("p1", proto::Side::Right), markdown("Plan", "first"))),
        Outcome::Done,
    );
    let latest = markdown("Latest", "the revised bytes");
    expect(&mut control, writing("paper", latest.clone()), Outcome::Done);
    let before = snapshot(&mut control);
    let before_record = record(&before, "paper");

    stop(&mut daemon, &mut control);
    assert!(file(&daemon, "paper").is_file());
    let state = saved_state(&daemon);
    assert_eq!(state["version"], 3);
    let saved = &state["panes"][1];
    assert_eq!(saved["canvas"]["opened_by"], "p1");
    assert!(saved.get("grid").is_none());
    assert!(saved.get("cwd").is_none());

    daemon.restart();
    let mut control = daemon.connect();
    let after = until_restored(&mut control, 2);
    let after_record = record(&after, "paper");
    assert_eq!(after.tabs, before.tabs);
    assert_eq!(after_record.canvas, before_record.canvas);
    assert_eq!(
        (&after_record.label, &after_record.callsign),
        (&before_record.label, &before_record.callsign)
    );
    assert!(after_record.quiet);
    assert_eq!(
        read(&mut control, "paper"),
        proto::CanvasRead { revision: 2, document: Some(latest.clone()) }
    );

    close(&mut control, "paper");
    until_saved(&daemon, "closed_panes");
    let state = saved_state(&daemon);
    assert_eq!(state["version"], 3, "a remembered canvas also sets the format floor");
    assert_eq!(state["closed_panes"][0]["pane"]["canvas"]["opened_by"], "p1");
    assert!(file(&daemon, "paper").is_file());
    let back = reopened(&mut control, before_record.callsign.as_deref().unwrap(), "reopened");
    assert_eq!(
        (&*back.pane, &*back.tab, back.made_tab, back.resumed),
        ("paper", "t1", false, false)
    );
    assert_eq!(snapshot(&mut control).tabs, before.tabs);
    assert_eq!(
        read(&mut control, "paper"),
        proto::CanvasRead { revision: 2, document: Some(latest) }
    );
}

#[test]
fn the_api_reopens_a_canvas_remembered_from_a_previous_run() {
    use crate::persistence::{stop, until_restored};
    use crate::reopen::close;

    let mut daemon = daemon();
    let mut control = daemon.connect();
    open(&mut control, "paper", "retained");
    close(&mut control, "paper");
    stop(&mut daemon, &mut control);
    daemon.restart();
    let mut control = daemon.connect();
    assert_eq!(until_restored(&mut control, 0).closed[0].pane, "paper");
    let mut api = daemon.api();
    let reopen = on_pane(api::pane_request::Request::Reopen(api::ReopenPane {
        pane: "paper".into(),
        operation: "reopen-paper".into(),
    }));
    let opened = api.ask(reopen.clone());
    assert_eq!(opened.outcome(), api::Outcome::Done, "{:?}", opened.answer);
    let Some(api::answer::Detail::Made(made)) = opened.answer.detail.as_ref() else {
        panic!("pane.reopen returned {:?}", opened.answer);
    };
    assert_eq!(made.pane, "paper");
    assert_eq!(api.ask(reopen).answer.detail, opened.answer.detail);
    assert_eq!(read(&mut control, "paper").document.unwrap().body, b"retained");
}

#[test]
fn closing_without_reopen_keeps_a_canvas_until_the_state_save_succeeds() {
    use crate::persistence::{state_file, until_saved};
    use crate::reopen::close;

    let daemon = daemon();
    let mut control = daemon.connect();
    open(&mut control, "paper", "retained");
    until_saved(&daemon, "paper");
    let temporary = state_file(&daemon).with_extension("json.tmp");
    assert!(!temporary.exists());
    std::fs::create_dir(&temporary).unwrap();
    let mut log = following(&daemon);
    let set = proto::SetReopenClosed { minutes: 0 };
    expect(&mut control, session(session_request::Request::SetReopenClosed(set)), Outcome::Done);
    close(&mut control, "paper");
    wait_in_log(&mut log, "daemon.state.not_written");
    assert!(snapshot(&mut control).closed.is_empty());
    assert!(file(&daemon, "paper").is_file());
    assert_eq!(saved_state(&daemon)["version"], 3);

    std::fs::remove_dir(temporary).unwrap();
    let set = proto::SetScrollback { bytes: Some(1_000_000) };
    expect(&mut control, session(session_request::Request::SetScrollback(set)), Outcome::Done);
    until("unreferenced document to be collected", || !file(&daemon, "paper").exists(), ());
    assert_eq!(saved_state(&daemon)["version"], 1);
}

#[test]
fn the_oldest_remembered_canvas_is_collected_when_evicted() {
    use crate::persistence::until_saved;
    use crate::reopen::close;

    let daemon = daemon();
    let mut control = daemon.connect();
    open(&mut control, "paper", "first");
    close(&mut control, "paper");
    until_saved(&daemon, "closed_panes");
    assert!(file(&daemon, "paper").exists());
    for index in 0..proto::CLOSED_PANES_KEPT {
        let name = format!("canvas-{index}");
        open(&mut control, &name, "new");
        close(&mut control, &name);
    }
    until("evicted document to be collected", || !file(&daemon, "paper").exists(), ());
    assert!(file(&daemon, "canvas-0").exists());
    assert_eq!(snapshot(&mut control).closed.len(), proto::CLOSED_PANES_KEPT);
}

#[test]
fn a_canvas_file_is_collected_when_its_reopen_deadline_passes() {
    use crate::persistence::{state_file, stop};
    use crate::reopen::close;
    use std::time::{Duration, SystemTime};

    let mut daemon = daemon();
    let mut control = daemon.connect();
    open(&mut control, "paper", "first");
    close(&mut control, "paper");
    stop(&mut daemon, &mut control);
    let mut kept = saved_state(&daemon);
    let expires =
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap() + Duration::from_secs(3);
    kept["closed_panes"][0]["until_ms"] =
        serde_json::json!(u64::try_from(expires.as_millis()).unwrap());
    std::fs::write(state_file(&daemon), kept.to_string()).unwrap();
    daemon.restart();
    assert_eq!(snapshot(&mut daemon.connect()).closed[0].pane, "paper");
    assert!(file(&daemon, "paper").is_file());
    until(
        "expired document to be collected without a request",
        || !file(&daemon, "paper").exists(),
        (),
    );
    assert!(snapshot(&mut daemon.connect()).closed.is_empty());
    assert_eq!(saved_state(&daemon)["version"], 1);
}

#[test]
fn a_startup_sweep_needs_a_readable_state_or_confirmed_absence() {
    use crate::persistence::{state_file, stop, until_restored};

    let mut daemon = daemon();
    let mut control = daemon.connect();
    open(&mut control, "paper", "retained");
    stop(&mut daemon, &mut control);
    let state = state_file(&daemon);
    let known = written(&state);
    let document = file(&daemon, "paper");
    let orphan = file(&daemon, "orphan");
    std::fs::copy(&document, &orphan).unwrap();
    std::fs::write(file(&daemon, "other"), "leave me alone").unwrap();

    std::fs::write(&state, "unreadable state data").unwrap();
    daemon.restart();
    let mut control = daemon.connect();
    assert!(snapshot(&mut control).panes.is_empty());
    assert!(document.is_file() && orphan.is_file());
    stop(&mut daemon, &mut control);

    let aside = std::fs::read_dir(daemon.root())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name().unwrap().to_string_lossy().starts_with("daemon.state.json.corrupt-")
        })
        .unwrap();
    daemon.restart();
    let mut control = daemon.connect();
    assert!(snapshot(&mut control).panes.is_empty());
    assert!(document.is_file() && orphan.is_file());
    let log = written(&daemon.root().join("daemon.log"));
    assert!(log.contains("daemon.canvas.collection_off"));
    assert!(log.contains(aside.file_name().unwrap().to_str().unwrap()));
    stop(&mut daemon, &mut control);

    std::fs::remove_file(aside).unwrap();
    std::fs::write(&state, known).unwrap();
    daemon.restart();
    let mut control = daemon.connect();
    until_restored(&mut control, 1);
    assert!(!orphan.exists());
    assert_eq!(read(&mut control, "paper").document.unwrap().body, b"retained");
    stop(&mut daemon, &mut control);

    std::fs::remove_file(state).unwrap();
    daemon.restart();
    assert!(snapshot(&mut daemon.connect()).panes.is_empty());
    assert!(!document.exists());
    assert_eq!(std::fs::read(file(&daemon, "other")).unwrap(), b"leave me alone");
}

#[test]
fn a_recovery_copy_preserves_the_documents_it_names_after_restart() {
    use crate::persistence::{state_file, stop, until_restored};

    let mut daemon = daemon();
    let mut control = daemon.connect();
    open(&mut control, "paper", "recover this");
    stop(&mut daemon, &mut control);
    let state = state_file(&daemon);
    let document = file(&daemon, "paper");
    let temporary = file(&daemon, ".away");
    std::fs::rename(&document, &temporary).unwrap();
    daemon.restart();
    let mut control = daemon.connect();
    until("restore to finish without its missing canvas", || !snapshot(&mut control).restoring, ());
    assert!(snapshot(&mut control).panes.is_empty());
    std::fs::rename(temporary, &document).unwrap();
    open(&mut control, "other", "unrelated");
    until(
        "the replacement state to be written",
        || {
            let saved = saved_state(&daemon);
            saved["panes"][0]["pane"] == "other"
        },
        (),
    );
    stop(&mut daemon, &mut control);
    daemon.restart();
    let mut control = daemon.connect();
    assert!(document.is_file());
    until_restored(&mut control, 1);
    assert_eq!(read(&mut control, "other").document.unwrap().body, b"unrelated");
    stop(&mut daemon, &mut control);
    assert!(written(&state).contains("other"));
}
