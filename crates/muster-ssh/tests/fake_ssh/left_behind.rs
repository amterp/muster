//! Masters an earlier Muster left running, and which of them a starting Muster ends.

use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use muster_ssh::{end_left_behind, tunnel_path};

use crate::fake;

#[test]
fn a_master_whose_muster_is_gone_is_ended_and_a_running_ones_is_not() {
    // The quit that left 0.10.1's master running kept its window's socket answering on the
    // devenv, so a pane there reached a window that had gone (kan a_2YAdjRtMB). A second window
    // on the same Mac holds a master to the same host under its own pid, and ending that one
    // would drop every pane it draws.
    let scratch = fake::Scratch::new();
    let gone = {
        let mut ended = Command::new("true").spawn().expect("true runs");
        ended.wait().expect("true finishes");
        ended.id()
    };
    let mut running = Command::new("sleep").arg("60").spawn().expect("sleep runs");
    let mut orphaned = stand_in_master(scratch.path(), gone);
    let mut kept = stand_in_master(scratch.path(), running.id());

    end_left_behind(scratch.path());

    let deadline = Instant::now() + Duration::from_secs(5);
    while orphaned.try_wait().expect("the stand-in can be asked").is_none() {
        assert!(Instant::now() < deadline, "the master of a Muster that is gone is still running");
        std::thread::sleep(Duration::from_millis(50));
    }
    for extension in ["ctl", "sock"] {
        let path = tunnel_path(scratch.path(), gone, "devenv", extension);
        assert!(!Path::new(&path).exists(), "{path} outlived its master");
    }

    let kept_control = tunnel_path(scratch.path(), running.id(), "devenv", "ctl");
    assert!(
        kept.try_wait().expect("the stand-in can be asked").is_none(),
        "the master of a Muster that is still running was ended: {:?}",
        fake::asked(&kept_control)
    );
    assert!(Path::new(&kept_control).exists(), "a running Muster's control path was removed");

    let _ = kept.kill();
    let _ = kept.wait();
    let _ = running.kill();
    let _ = running.wait();
}

/// A process standing in for the master of the tunnel `devenv` that the Muster with this pid
/// opened, with both its paths where that Muster would have put them.
fn stand_in_master(directory: &Path, pid: u32) -> Child {
    let control = tunnel_path(directory, pid, "devenv", "ctl");
    std::fs::write(&control, "").expect("the control path can be made");
    std::fs::write(tunnel_path(directory, pid, "devenv", "sock"), "")
        .expect("the local socket can be made");
    let master = Command::new("sleep").arg("600").spawn().expect("sleep runs");
    let state = format!("{control}.d");
    std::fs::create_dir_all(&state).expect("the stand-in's state can be made");
    std::fs::write(format!("{state}/master"), master.id().to_string())
        .expect("the stand-in's pid can be written");
    master
}
