//! A pane asked for by a name it had before a rename.
//!
//! `$MUSTER_PANE_NAME` keeps the name a pane was started with, so an agent that addresses itself
//! by it after its pane was renamed used to be told the pane had closed or came from an older
//! window (kan a_2izzZ0Sag). The window now says what the pane is called instead.

use muster::proto::{
    OpenWindow, ReadPane, RenamePane, Request, Response, Startup, request, response,
};
use muster_daemon_proto::pane_request;
use muster_harness::requests::{create, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn a_pane_asked_for_by_its_name_from_before_a_rename_is_answered_with_its_name_now() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let named =
        pane_request::Create { label: Some("kelp".to_string()), ..create("p1", in_new_tab("t1")) };
    make(&mut daemon.connect(), named);

    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to list the pane by the name it was made with",
        || reads("kelp"),
        || format!("a read of kelp answered {:?}", read("kelp").payload),
    );

    assert_ok(&answer(request::Payload::RenamePane(RenamePane {
        pane_id: "p1".to_string(),
        name: "🤖 A".to_string(),
        ..RenamePane::default()
    })));
    until(
        "the window to hear of the rename",
        || reads("🤖 A"),
        || format!("a read of 🤖 A answered {:?}", read("🤖 A").payload),
    );

    let refused = read("kelp");
    let Some(response::Payload::Failure(failure)) = refused.payload else {
        panic!("a read by a name nothing has now was answered with {:?}", refused.payload);
    };
    assert!(
        failure.reason.contains("🤖 A") && failure.reason.contains("p1"),
        "the refusal does not say what the pane is called now: {}",
        failure.reason
    );
}

fn read(pane: &str) -> Response {
    answer(request::Payload::ReadPane(ReadPane {
        pane_id: pane.to_string(),
        ..ReadPane::default()
    }))
}

fn reads(pane: &str) -> bool {
    matches!(read(pane).payload, Some(response::Payload::PaneText(_)))
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => panic!("the core refused: {}", failure.reason),
        None => panic!("the core answered with no payload"),
        Some(_) => {}
    }
}
