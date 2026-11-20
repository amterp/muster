use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

use crate::fake;

#[test]
fn a_master_without_the_forward_is_ended_and_replaced() {
    let scratch = fake::Scratch::new();
    let forward = fake::forward(scratch.path());
    let control = forward.control_path.clone();
    let local = forward.local_socket.clone();
    let tunnel = fake::open(forward);

    // The forwarding process can leave a socket file that still refuses connections.
    std::fs::remove_file(&local).expect("the fake's placeholder can be replaced");
    drop(UnixListener::bind(&local).expect("the local socket can be bound"));
    assert_eq!(
        UnixStream::connect(&local).unwrap_err().kind(),
        std::io::ErrorKind::ConnectionRefused
    );
    fake::replace_master(&control);

    fake::until("a confirmed replacement master", &control, Duration::from_secs(10), || {
        fake::count(&control, "run") >= 1
    });
    let asked = fake::asked(&control);
    let takeover = asked.iter().position(|line| line == "foreign check");
    let exit = asked.iter().position(|line| line == "control exit");
    let reopen = asked.iter().rposition(|line| line == "master");
    assert!(
        matches!((takeover, exit, reopen), (Some(check), Some(exit), Some(reopen)) if check < exit && exit < reopen),
        "the supervisor should end the foreign master before reopening: {asked:?}"
    );
    assert!(std::path::Path::new(&local).is_file(), "the new master made its forward");
    drop(tunnel);
}
