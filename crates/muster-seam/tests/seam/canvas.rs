use std::path::Path;

use muster::proto::{
    CanvasDocument, CanvasKind, CanvasPage, OpenCanvas, OpenWindow, ReadCanvas, ReadWindow,
    Request, Response, Startup, WriteCanvas, open_canvas, request, response,
};
use muster_daemon_proto::{self as daemon_proto, pane_request};
use muster_harness::Daemon;
use muster_harness::requests::snapshot;
use muster_harness::until_some;
use prost::Message;

#[test]
fn canvas_can_be_written_by_its_opener_and_read_as_original_bytes_or_a_page() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let opener = start(&daemon.muster_config());
    assert_eq!(keyboard(), opener);

    let made = made(answer(open(&opener, false), &opener));
    assert!(!made.callsign.is_empty());
    assert_eq!(keyboard(), opener);
    let body = b"# New heading\n\n";
    let write = answer(write("", body), &opener);
    assert!(matches!(write, response::Payload::CanvasWritten(written) if written.revision == 2));
    let read = answer(read(&made.callsign), "");
    let response::Payload::CanvasRead(read) = read else { panic!("read returned {read:?}") };
    assert_eq!(read.revision, 2);
    assert_eq!(read.document.unwrap(), document(body));

    let page = answer(
        request::Payload::CanvasPage(CanvasPage {
            pane_id: made.pane_id,
            daemon_id: "local".into(),
        }),
        "",
    );
    let response::Payload::CanvasRendered(page) = page else { panic!("page returned {page:?}") };
    assert_eq!(page.revision, 2);
    assert!(page.html.contains("<h1>New heading</h1>"), "{}", page.html);
    assert_eq!(page.load_schemes, ["data", "blob", "about"]);
    assert_eq!(page.external_link_schemes, ["http", "https", "mailto"]);
    assert!(!page.scripts_enabled);
}

#[test]
fn canvas_tab_refuses_a_source_pane_the_window_no_longer_holds() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let opener = start(&daemon.muster_config());
    let missing = "pclosedpane";
    let result = answer(
        request::Payload::OpenCanvas(OpenCanvas {
            placement: Some(open_canvas::Placement::NewTab(open_canvas::NewTab {
                pane_id: missing.into(),
                ..open_canvas::NewTab::default()
            })),
            document: Some(document(b"report")),
            ..OpenCanvas::default()
        }),
        &opener,
    );
    let response::Payload::Failure(failure) = result else { panic!("{result:?}") };
    assert!(failure.reason.contains(missing), "{}", failure.reason);
    assert_eq!(snapshot(&mut daemon.connect()).panes.len(), 1);
}

#[test]
fn canvas_refuses_all_three_operations_when_the_daemon_says_protocol_1_16() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let relay = daemon.saying_protocol_minor(16);
    let opener = start(&relay.muster_config());
    for payload in [open(&opener, false), write(&opener, b"replace"), read(&opener)] {
        let response::Payload::Failure(failure) = answer(payload, &opener) else {
            panic!("a canvas operation succeeded on protocol 1.16");
        };
        for expected in ["local", "1.16"] {
            assert!(failure.reason.contains(expected), "{}", failure.reason);
        }
    }
    assert_eq!(snapshot(&mut daemon.connect()).panes.len(), 1);
}

#[test]
fn canvas_write_with_a_lost_answer_is_unanswered_and_lands_once() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let relay = daemon.hanging_up_before_answers_where(|asked| {
        matches!(
            asked.service,
            Some(daemon_proto::request::Service::Pane(daemon_proto::PaneRequest {
                request: Some(pane_request::Request::WriteCanvas(_)),
            }))
        )
    });
    let opener = start(&relay.muster_config());
    let made = made(answer(open(&opener, false), &opener));
    let reply = answer(write("", b"might have landed"), &opener);
    assert!(matches!(reply, response::Payload::Unanswered(_)), "{reply:?}");

    let held = snapshot(&mut daemon.connect())
        .panes
        .into_iter()
        .find(|pane| pane.pane == made.pane_id)
        .expect("the canvas stayed open");
    assert_eq!(held.canvas.unwrap().revision, 2, "a second write would have advanced the revision");
}

fn open(beside: &str, take_focus: bool) -> request::Payload {
    request::Payload::OpenCanvas(OpenCanvas {
        placement: Some(open_canvas::Placement::Beside(open_canvas::Beside {
            pane_id: beside.into(),
            side: "right".into(),
            ratio: 0.5,
            ..open_canvas::Beside::default()
        })),
        document: Some(document(b"original")),
        take_focus,
        ..OpenCanvas::default()
    })
}

fn write(pane: &str, body: &[u8]) -> request::Payload {
    request::Payload::WriteCanvas(WriteCanvas {
        pane_id: pane.into(),
        document: Some(document(body)),
        ..WriteCanvas::default()
    })
}

fn read(pane: &str) -> request::Payload {
    request::Payload::ReadCanvas(ReadCanvas { pane_id: pane.into(), ..ReadCanvas::default() })
}

fn document(body: &[u8]) -> CanvasDocument {
    CanvasDocument {
        kind: CanvasKind::Markdown.into(),
        body: body.to_vec(),
        title: "Report".into(),
        ..CanvasDocument::default()
    }
}

fn keyboard() -> String {
    let response::Payload::Window(window) =
        answer(request::Payload::ReadWindow(ReadWindow::default()), "")
    else {
        panic!("the window did not answer")
    };
    window.view.map_or_else(String::new, |view| view.keyboard_pane_id)
}

fn made(response: response::Payload) -> muster::proto::Made {
    let response::Payload::Made(made) = response else { panic!("open returned {response:?}") };
    made
}

fn start(config: &Path) -> String {
    for request in [
        request::Payload::Startup(Startup {
            config_path: config.to_string_lossy().into_owned(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        let response = answer(request, "");
        assert!(
            matches!(response, response::Payload::Ok(_) | response::Payload::Opened(_)),
            "{response:?}"
        );
    }
    until_some("the window to have its first terminal pane", || {
        let pane = keyboard();
        (!pane.is_empty()).then_some(pane)
    })
}

fn answer(payload: request::Payload, from: &str) -> response::Payload {
    let mut request = Request::new(payload);
    request.from_pane = from.into();
    let reply = muster::dispatch(&request.encode_to_vec());
    Response::decode(reply.as_slice()).expect("a response").payload.expect("an answer")
}
