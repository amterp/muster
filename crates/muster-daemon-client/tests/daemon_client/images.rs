use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use muster_core::input::ImageCancellation;
use muster_core::mirror::backend::PaneId;
use muster_daemon_client::images;
use muster_daemon_proto as proto;
use muster_harness::Daemon;
use muster_harness::requests::{create, in_new_tab, make};

#[test]
fn a_separate_upload_connection_stores_the_bytes_on_the_panes_machine() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.resize(5 << 20, 0x55);
    let path = images::store(
        daemon.socket_path(),
        &PaneId::new("p1"),
        png.clone(),
        &ImageCancellation::default(),
    )
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), png);
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(
        Path::new(&path).parent(),
        Some(daemon.socket_path().with_extension("images").as_path())
    );
    assert_eq!(
        control
            .ask(proto::request::Service::Session(proto::SessionRequest {
                request: Some(proto::session_request::Request::Snapshot(
                    proto::session_request::Snapshot {}
                )),
            }))
            .outcome(),
        proto::Outcome::Done
    );
}

#[test]
fn an_upload_to_a_missing_pane_creates_no_image_cache() {
    let daemon = Daemon::start_built();
    let result = images::store(
        daemon.socket_path(),
        &PaneId::new("gone"),
        b"\x89PNG\r\n\x1a\ncontent".to_vec(),
        &ImageCancellation::default(),
    );
    assert!(result.unwrap_err().contains("no pane"));
    assert!(!daemon.socket_path().with_extension("images").exists());
}

#[test]
fn a_slow_final_answer_does_not_fail_an_upload_that_reached_the_daemon() {
    let daemon = Daemon::start_built();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    let relay = daemon.delaying_answers_where(
        |request| {
            matches!(
                &request.service,
                Some(proto::request::Service::Pane(proto::PaneRequest {
                    request: Some(proto::pane_request::Request::StoreImage(_)),
                }))
            )
        },
        Duration::from_secs(11),
    );
    let png = b"\x89PNG\r\n\x1a\nimage".to_vec();
    let path = images::store(
        relay.socket_path(),
        &PaneId::new("p1"),
        png.clone(),
        &ImageCancellation::default(),
    )
    .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), png);
}
