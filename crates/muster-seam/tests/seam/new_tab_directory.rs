//! Where a new tab starts when the pane it was made beside sits in a directory that is gone.
//!
//! A new tab starts in the focused pane's directory, so a shell left in a removed checkout used
//! to make every cmd+T refuse, eight times in fifteen seconds before anybody worked out why
//! (kan a_2jzG4pkNf). Nobody typed that directory, so the tab opens at home; a directory somebody
//! did type is still refused, since that one is what they asked for.

use muster::proto::{
    CreateTab, OpenWindow, ReadWindow, Request, Response, Startup, request, response,
};
use muster_harness::requests::{create, in_new_tab, make, snapshot};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn a_new_tab_beside_a_pane_whose_directory_is_gone_opens_at_home() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let gone = daemon.root().join("gone");
    std::fs::create_dir_all(&gone).unwrap();
    let mut control = daemon.connect();
    let mut first = create("p1", in_new_tab("t1"));
    first.cwd = Some(gone.display().to_string());
    make(&mut control, first);
    std::fs::remove_dir(&gone).unwrap();

    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    // Waited for in the window rather than the daemon: the directory the tab inherits is the one
    // the window has heard its pane is in, and a window that had heard nothing yet would send
    // none and pass this test without ever inheriting a directory.
    until(
        "the window to hold the pane in the removed directory",
        || window_panes().iter().any(|pane| pane == "p1"),
        || format!("the window holds {:?}", window_panes()),
    );

    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        pane_id: "p1".to_string(),
        take_focus: true,
        ..CreateTab::default()
    })));
    let home = daemon.root().join("home").display().to_string();
    let cwds: Vec<String> = snapshot(&mut control).panes.into_iter().map(|pane| pane.cwd).collect();
    assert!(cwds.contains(&home), "the new tab's pane did not start at home, {home}: {cwds:?}");

    let typed = answer(request::Payload::CreateTab(CreateTab {
        cwd: gone.display().to_string(),
        ..CreateTab::default()
    }));
    match typed.payload {
        Some(response::Payload::Failure(failure)) => assert!(
            failure.reason.contains(&gone.display().to_string()),
            "the refusal does not name the directory that was typed: {}",
            failure.reason
        ),
        other => panic!("a tab in a typed directory that is gone was not refused: {other:?}"),
    }
}

fn window_panes() -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => {
            window.panes.into_iter().map(|pane| pane.pane_id).collect()
        }
        _ => Vec::new(),
    }
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
