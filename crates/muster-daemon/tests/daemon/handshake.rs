//! Every connection opens with a handshake, and the daemon serves only clients of its major.

use std::io::Write;
use std::os::unix::net::UnixStream;

use crate::support::*;
use muster_daemon_proto::connection;
use muster_daemon_proto::version::PROTOCOL;

#[test]
fn a_control_client_is_welcomed_with_the_daemon_it_reached() {
    let daemon = daemon();
    let control = daemon.connect();
    let welcome = control.welcome();
    assert_eq!(welcome.protocol, Some(PROTOCOL));
    assert_eq!(welcome.install, muster_daemon_proto::install::INSTALL);
    assert_eq!(welcome.pid, daemon.pid());
    assert!(!welcome.daemon_version.is_empty());
    assert_eq!(welcome.launch, "", "nobody started this daemon with --launch");
}

/// A daemon started through Launch Services is not its starter's child, so the starter knows it
/// from a rival's by the token it was started with.
#[test]
fn a_daemon_repeats_the_token_it_was_launched_with() {
    let root = std::path::PathBuf::from(format!("/tmp/muster-test/h{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("home")).unwrap();
    let socket = root.join("d.sock");
    let mut process = std::process::Command::new(env!("CARGO_BIN_EXE_muster-daemon"))
        .args(["--launch", "--- a start of its own ---", "--data", DAEMON_DATA, "--socket"])
        .arg(&socket)
        .env_clear()
        .env("HOME", root.join("home"))
        .env("SHELL", "/bin/sh")
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let welcome = until_some("the daemon to answer", || {
        connection::connect(&socket, proto::ConnectionKind::Control, "test").ok()
    })
    .1;
    let _ = process.kill();
    let _ = process.wait();
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(welcome.launch, "--- a start of its own ---");
}

#[test]
fn a_client_of_another_major_is_refused_with_both_versions() {
    let daemon = daemon();
    let mut stream = UnixStream::connect(daemon.socket_path()).unwrap();
    let hello = proto::Hello {
        protocol: Some(proto::Version { major: PROTOCOL.major + 1, minor: 0 }),
        kind: proto::ConnectionKind::Control.into(),
        client: "from the future".to_string(),
    };
    connection::send(&mut stream, &hello).unwrap();
    let answer = connection::receive::<proto::HelloAnswer>(&mut stream).unwrap().unwrap();
    let Some(proto::hello_answer::Answer::Refused(refused)) = answer.answer else {
        panic!("a client of another major was not refused: {answer:?}");
    };
    assert_eq!(refused.daemon, Some(PROTOCOL));
    assert!(refused.reason.contains("major"), "{}", refused.reason);
    assert!(
        connection::receive::<proto::ControlMessage>(&mut stream).unwrap().is_none(),
        "the daemon hangs up after refusing"
    );
}

#[test]
fn every_kind_of_connection_is_welcomed() {
    let daemon = daemon();
    for kind in [
        proto::ConnectionKind::Control,
        proto::ConnectionKind::Stream,
        proto::ConnectionKind::Input,
    ] {
        if let Err(error) = connection::connect(daemon.socket_path(), kind, "test") {
            panic!("a {kind:?} connection was not welcomed: {error:?}");
        }
    }
}

#[test]
fn something_that_is_not_muster_gets_no_welcome() {
    let daemon = daemon();
    let mut stream = UnixStream::connect(daemon.socket_path()).unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
    let answer = connection::receive::<proto::HelloAnswer>(&mut stream);
    assert!(
        !matches!(
            answer,
            Ok(Some(proto::HelloAnswer { answer: Some(proto::hello_answer::Answer::Welcome(_)) }))
        ),
        "an HTTP request was welcomed: {answer:?}"
    );
    // And the daemon is still there for everyone else.
    daemon.connect();
}
