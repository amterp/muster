//! The daemon started on a real remote machine, reached through a real ssh master.
//!
//! Out of the default gate, and marked `#[ignore]` to keep it there: it needs the devenv
//! container (`docs/testing.md`). `./dev --ssh` brings the container up, places this build's
//! Linux daemon where a remote machine keeps it, and runs this.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use muster_daemon_client::environment::for_far_daemon;
use muster_daemon_client::launch::{Reached, stop};
use muster_daemon_client::remote::{Installed, ensure_running};
use muster_daemon_proto as proto;
use muster_harness::Control;
use muster_harness::requests::*;
use muster_ssh::{Forward, Tunnel, remote_environment};

/// Where the container is and how to reach it, from `./dev --ssh`.
fn devenv() -> (String, Vec<String>) {
    let host = std::env::var("MUSTER_DEVENV_HOST").expect(
        "MUSTER_DEVENV_HOST is unset, so this test has no machine to talk to. Run it through \
         ./dev --ssh, which starts the container and sets it.",
    );
    let options = std::env::var("MUSTER_DEVENV_SSH_OPTIONS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    (host, options)
}

#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn the_daemon_over_there_is_started_then_adopted_and_serves_a_pane() {
    let (host, options) = devenv();
    let far = remote_environment(&host, &options).unwrap();
    let installed = Installed::on(&far).expect("the container has a HOME");
    let environment = for_far_daemon(&far);
    let temporary = std::env::temp_dir();
    let tunnel = Tunnel::open(
        Forward {
            host,
            options,
            control_path: temporary.join("muster-devenv-client.ctl").to_string_lossy().into(),
            local_socket: temporary.join("muster-devenv-client.sock").to_string_lossy().into(),
            remote_socket: installed.socket.to_string_lossy().into(),
        },
        Arc::new(|_| {}),
    )
    .expect("the tunnel opens");
    let local = Path::new(tunnel.local_socket_path());
    // One left from an earlier run is somebody's in real life; here it would make the start
    // below an adoption.
    let _ = stop(local, Duration::from_secs(10));

    let (reached, started) =
        ensure_running(&tunnel.remote(), &installed, local, &environment).unwrap();
    assert_eq!(reached, Reached::Started);

    let mut control = Control::connect(local);
    make(
        &mut control,
        proto::pane_request::Create {
            command: Some("echo over-there".into()),
            ..create("p1", in_new_tab("t1"))
        },
    );
    until_text(&mut control, "p1", "over-there");

    let (reached, adopted) =
        ensure_running(&tunnel.remote(), &installed, local, &environment).unwrap();
    assert_eq!(reached, Reached::Adopted);
    assert_eq!(adopted.instance, started.instance);

    stop(local, Duration::from_secs(10)).unwrap();
}
