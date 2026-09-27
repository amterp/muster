//! Starting this machine's daemon, or adopting the one already running.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use muster_daemon_client::launch::{Launch, Reached, ensure_running, stop};
use muster_harness::{DAEMON_DATA, built_daemon};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A scratch directory with a socket path short enough to bind, and whatever daemon a test
/// started there stopped when it ends.
struct Scratch {
    binary: PathBuf,
    root: PathBuf,
    socket: PathBuf,
    environment: BTreeMap<String, String>,
}

impl Scratch {
    fn new() -> Scratch {
        let root = PathBuf::from(format!(
            "/tmp/muster-test/l{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        let environment = BTreeMap::from([
            ("HOME".to_string(), root.join("home").display().to_string()),
            ("PATH".to_string(), std::env::var("PATH").unwrap_or_default()),
            ("SHELL".to_string(), "/bin/sh".to_string()),
        ]);
        Scratch { binary: built_daemon(), socket: root.join("d.sock"), root, environment }
    }

    fn launch(&self) -> Launch<'_> {
        Launch {
            binary: &self.binary,
            data: Some(DAEMON_DATA.as_ref()),
            socket: &self.socket,
            environment: &self.environment,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = stop(&self.socket, Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_daemon_is_started_once_and_adopted_after() {
    let scratch = Scratch::new();
    let (reached, started) = ensure_running(&scratch.launch()).unwrap();
    assert_eq!(reached, Reached::Started);
    let (reached, adopted) = ensure_running(&scratch.launch()).unwrap();
    assert_eq!(reached, Reached::Adopted);
    assert_eq!((adopted.instance, adopted.pid), (started.instance, started.pid));
}

#[test]
fn two_starting_at_once_end_with_one_daemon() {
    let scratch = Scratch::new();
    let launched = std::thread::scope(|scope| {
        let first = scope.spawn(|| ensure_running(&scratch.launch()).unwrap());
        let second = scope.spawn(|| ensure_running(&scratch.launch()).unwrap());
        [first.join().unwrap(), second.join().unwrap()]
    });
    let [(_, first), (_, second)] = launched;
    assert_eq!((first.instance, first.pid), (second.instance, second.pid));
}

#[test]
fn a_daemon_that_dies_at_once_says_why() {
    let scratch = Scratch::new();
    let broken = scratch.root.join("broken");
    std::fs::write(&broken, "#!/bin/sh\necho 'no data directory here' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&broken, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let launch = Launch { binary: &broken, ..scratch.launch() };
    let error = ensure_running(&launch).unwrap_err();
    assert!(error.contains("no data directory here"), "{error}");
}

#[test]
fn a_stopped_daemon_stops_answering() {
    let scratch = Scratch::new();
    ensure_running(&scratch.launch()).unwrap();
    stop(&scratch.socket, Duration::from_secs(5)).unwrap();
    let (reached, _) = ensure_running(&scratch.launch()).unwrap();
    assert_eq!(reached, Reached::Started, "nothing was left answering");
}
