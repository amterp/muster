//! A followed daemon's own log records reach this run's log, saying which daemon wrote them.
//!
//! A binary of its own rather than a module of `daemon_client`, because it installs this
//! process's log sink, which every test in a binary shares.

use std::sync::{Arc, Mutex};

use muster_core::diagnostics::log::{self, LogLevel, LogRecord, LogSink};
use muster_core::mirror::Mirror;
use muster_daemon_client::follow::{Follower, Following};
use muster_harness::{Daemon, until_some};

#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<String>>>);

impl LogSink for Lines {
    fn write(&self, _record: &LogRecord) {}

    fn write_line(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_string());
    }
}

#[test]
fn a_daemons_records_arrive_in_this_runs_log_naming_it() {
    let lines = Lines::default();
    log::install(Box::new(lines.clone()), "test", LogLevel::Debug);
    let daemon = Daemon::start_built();
    let _follower = Follower::start(
        Following {
            socket: daemon.socket_path().to_path_buf(),
            client: "test".to_string(),
            daemon: "laptop".to_string(),
            remote: false,
        },
        Arc::new(Mutex::new(Mirror::new())),
        Arc::new(|_| {}),
    )
    .unwrap();

    let relayed = until_some("a record of the daemon's own", || {
        lines.0.lock().unwrap().iter().find(|line| line.contains("\"process\":\"daemon\"")).cloned()
    });
    assert!(relayed.ends_with(",\"daemon\":\"laptop\"}\n"), "{relayed}");
    assert!(!relayed.contains("daemon_mono_ns"), "a local daemon's clock is this machine's");
}
