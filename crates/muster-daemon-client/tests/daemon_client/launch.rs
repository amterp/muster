//! Starting this machine's daemon, or adopting the one already running.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use conformance::{CaseError, Conformance, fields};
use muster_daemon_client::launch::{
    Launch, Reached, Route, ensure_running, open_arguments, route, stop,
};
use muster_harness::{DAEMON_DATA, built_daemon};
use serde_json::{Value, json};

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

/// A started daemon leads a session of its own, so the app quitting or the terminal it was
/// started from closing cannot take its agents with it.
#[test]
fn a_started_daemon_leads_a_session_of_its_own() {
    let scratch = Scratch::new();
    let (reached, started) = ensure_running(&scratch.launch()).unwrap();
    assert_eq!(reached, Reached::Started);
    let pid = libc::pid_t::try_from(started.pid).unwrap();
    // SAFETY: getsid only reads another process's session id.
    let session = unsafe { libc::getsid(pid) };
    assert_eq!(session, pid, "the daemon leads its own session");
}

#[test]
fn the_first_daemon_on_a_machine_makes_its_own_directory() {
    let mut scratch = Scratch::new();
    scratch.socket = scratch.root.join("fresh").join("daemon").join("d.sock");
    let (reached, _) = ensure_running(&scratch.launch()).unwrap();
    assert_eq!(reached, Reached::Started);
}

#[test]
fn two_starting_at_once_end_with_one_daemon() {
    let scratch = Scratch::new();
    let launched = std::thread::scope(|scope| {
        let first = scope.spawn(|| ensure_running(&scratch.launch()).unwrap());
        let second = scope.spawn(|| ensure_running(&scratch.launch()).unwrap());
        [first.join().unwrap(), second.join().unwrap()]
    });
    let [(first_reached, first), (second_reached, second)] = launched;
    assert_eq!((first.instance, first.pid), (second.instance, second.pid));
    let started =
        [first_reached, second_reached].iter().filter(|r| **r == Reached::Started).count();
    assert_eq!(started, 1, "only the start whose daemon answered says it started one");
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

/// Two starts on one socket share its stderr file, and the second does not erase what the
/// first one's daemon had already said.
#[test]
fn two_daemons_dying_at_once_each_say_why() {
    let scratch = Scratch::new();
    let broken = scratch.root.join("broken");
    std::fs::write(&broken, "#!/bin/sh\necho \"$WHO: no data directory\" >&2\nsleep 0.5\nexit 1\n")
        .unwrap();
    std::fs::set_permissions(&broken, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let said_by = |who: &str| {
        let mut environment = scratch.environment.clone();
        environment.insert("WHO".to_string(), who.to_string());
        environment
    };
    let (first, second) = (said_by("first"), said_by("second"));
    let errors = std::thread::scope(|scope| {
        let one = scope.spawn(|| {
            ensure_running(&Launch { binary: &broken, environment: &first, ..scratch.launch() })
                .unwrap_err()
        });
        std::thread::sleep(Duration::from_millis(200));
        let two = scope.spawn(|| {
            ensure_running(&Launch { binary: &broken, environment: &second, ..scratch.launch() })
                .unwrap_err()
        });
        [one.join().unwrap(), two.join().unwrap()]
    });
    assert!(errors[0].contains("first: no data directory"), "{}", errors[0]);
    assert!(errors[1].contains("second: no data directory"), "{}", errors[1]);
}

#[test]
fn a_stopped_daemon_stops_answering() {
    let scratch = Scratch::new();
    ensure_running(&scratch.launch()).unwrap();
    stop(&scratch.socket, Duration::from_secs(5)).unwrap();
    let (reached, _) = ensure_running(&scratch.launch()).unwrap();
    assert_eq!(reached, Reached::Started, "nothing was left answering");
}

/// A daemon that accepts and never answers, as a stopped or deadlocked one does, fails the
/// dial within the handshake's bound and is never taken for an empty socket.
#[test]
fn a_daemon_that_accepts_and_never_answers_is_not_waited_on_for_ever() {
    let scratch = Scratch::new();
    let listener = std::os::unix::net::UnixListener::bind(&scratch.socket).unwrap();
    let held = std::thread::spawn(move || listener.accept().map(|(stream, _)| stream));
    let started = std::time::Instant::now();
    let error = ensure_running(&scratch.launch()).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(8), "took {:?}", started.elapsed());
    assert!(error.contains("is not answering"), "{error}");
    drop(held.join().unwrap());
}

/// How a daemon is started from its path, and what `open` is given when it is a bundle's. Cases
/// live in corpus/conformance/daemon-launch.json. A route is decided on macOS only.
#[cfg(target_os = "macos")]
#[test]
fn daemon_launch_conformance() {
    let corpus = Conformance::load("daemon-launch.json");
    let ran = corpus.run(|given| {
        let text = |name: &str| {
            given
                .get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| CaseError::new(format!("`{name}` is missing")))
        };
        let binary = PathBuf::from(text("binary")?);
        let socket = PathBuf::from(text("socket")?);
        let stderr = PathBuf::from(text("stderr")?);
        let data = given.get("data").and_then(Value::as_str).map(PathBuf::from);
        let environment: BTreeMap<String, String> = given
            .get("environment")
            .and_then(Value::as_object)
            .map(|pairs| {
                pairs
                    .iter()
                    .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().into()))
                    .collect()
            })
            .unwrap_or_default();
        let launch = Launch {
            binary: &binary,
            data: data.as_deref(),
            socket: &socket,
            environment: &environment,
        };
        Ok(match route(&binary) {
            Route::Spawn => fields([("route", Some(json!("spawn")))]),
            Route::Open { bundle } => {
                let open: Vec<String> = open_arguments(&bundle, &launch, &stderr, text("marker")?)
                    .iter()
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect();
                fields([
                    ("route", Some(json!("launch_services"))),
                    ("bundle", Some(json!(bundle.display().to_string()))),
                    ("open", Some(json!(open))),
                ])
            }
        })
    });
    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
