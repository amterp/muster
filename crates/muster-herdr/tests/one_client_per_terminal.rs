//! What a herdr client that loses its terminal to a takeover is told, measured.
//!
//! One client may hold a herdr terminal. That is the fact under two things Muster cannot do
//! today: two windows cannot show one pane, and a pane cannot be handed from one window to
//! another (kan `a_2IZ6Of6JP`). What decides how expensive the second one is is what the client
//! that *lost* the terminal sees - if its stream ends, a window can say the pane went somewhere;
//! if it sees nothing, the window renders a lie until something else notices.
//!
//! Nothing above the adapter reads this today. It is here because a recorded fact settles a card
//! that reasoning could not, and because it is the kind of claim that goes stale silently: a
//! herdr that starts telling a displaced client turns this test red rather than leaving a
//! sentence in `observations/` that nobody re-checks.
//!
//! The stream itself is not the JSON API - it is `herdr terminal session control` over stdio,
//! which is what `muster-bridge` runs - so this spawns the same command the bridge does rather
//! than going through `HerdrClient`.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use herdr_harness::Daemon;
use serde_json::json;

/// How long to wait for a displaced client to notice, before recording that it did not.
///
/// The measurement is a negative as much as a positive - "it saw nothing" is an answer, and one
/// only elapsed time can give. A second is comfortably past what was recorded: three runs ended
/// the first client's stream 258 to 259 ms after the second client was *spawned*, and most of
/// that is a process starting rather than herdr deciding.
const NOTICING: Duration = Duration::from_secs(1);

#[test]
fn a_client_that_loses_its_terminal_sees_its_stream_end() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));
    let pane = only_pane(&daemon);

    let mut first = attach(&daemon, &pane, false);
    let mut painting = first.stdout.take().expect("the client's stdout is piped");
    // Frames before the takeover, so that a stream which ends here is one that was live rather
    // than one that never started.
    let mut opening = [0u8; 1];
    painting.read_exact(&mut opening).expect("the first client's stream carries frames");

    let second = attach(&daemon, &pane, true);

    // Read to the end of the first client's stream. This is the whole measurement: whether it
    // ends at all, and how long it takes.
    let began = Instant::now();
    let ended = std::thread::spawn(move || {
        let mut rest = Vec::new();
        let _ = painting.read_to_end(&mut rest);
    });
    let mut noticed = false;
    while began.elapsed() < NOTICING {
        if ended.is_finished() {
            noticed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // Killed and reaped, both of them: a test that left two clients holding a scratch daemon's
    // terminal would leave them behind when the daemon's directory went.
    for mut client in [first, second] {
        let _ = client.kill();
        let _ = client.wait();
    }

    assert!(
        noticed,
        "the client that lost the terminal was still reading {NOTICING:?} later. That is the \
         answer `a_2IZ6Of6JP` was waiting for and it is the expensive one: a window whose pane \
         was taken renders what it last painted, with nothing on the stream to say so, so \
         handing a pane between windows needs a channel of its own to announce it. Record the \
         new answer in `docs/observations/herdr-0.8.0.md` rather than deleting this test."
    );
}

/// One client running the same command a pane's bridge runs.
fn attach(daemon: &Daemon, pane: &str, takeover: bool) -> Child {
    let mut command = Command::new(herdr_harness::binary());
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HERDR_SOCKET_PATH", daemon.socket_path())
        .args(["terminal", "session", "control", pane])
        .args(["--cols", "80", "--rows", "24"]);
    if takeover {
        command.arg("--takeover");
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the pinned herdr runs")
}

fn only_pane(daemon: &Daemon) -> String {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    snapshot["snapshot"]["panes"]
        .as_array()
        .and_then(|panes| panes.first())
        .and_then(|pane| pane["pane_id"].as_str())
        .expect("the workspace brought a pane with it")
        .to_string()
}
