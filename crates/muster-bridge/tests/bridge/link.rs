//! What a bridge tells the window about itself, and what it draws, against the real daemon.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use muster_core::bridge_link::Report;
use muster_core::respawn::Ending;
use muster_daemon_client::input::Input;
use muster_daemon_proto::{self as proto, input_event};
use muster_harness::requests::{create, in_new_tab, make};
use muster_harness::{Daemon, PATIENCE, built_daemon, until};

/// A window's end of one pane's link: every report a bridge sends, as it arrives.
struct Window {
    reports: Receiver<Report>,
    socket: PathBuf,
}

impl Window {
    fn bind(daemon: &Daemon, name: &str) -> Window {
        let socket = daemon.root().join(format!("{name}.sock"));
        let listener = UnixListener::bind(&socket).unwrap();
        let (tell, reports) = channel();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if let Some(report) = Report::parse(&line) {
                        let _ = tell.send(report);
                    }
                }
            }
        });
        Window { reports, socket }
    }

    fn next(&self) -> Report {
        self.reports.recv_timeout(PATIENCE).expect("the bridge said something")
    }
}

fn bridge(daemon: &Daemon, pane: &str, window: &Window, takeover: bool) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_muster-bridge"));
    command
        .arg(pane)
        .arg("--daemon-socket")
        .arg(daemon.socket_path())
        .arg("--app-socket")
        .arg(&window.socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if takeover {
        command.arg("--takeover");
    }
    command.spawn().unwrap()
}

fn exiting(report: Report) -> Ending {
    match report {
        Report::Exiting(ended) => ended.ending,
        other => panic!("expected the bridge to say why it exits, and it said {other:?}"),
    }
}

/// What a surface would be showing, as it arrives on the bridge's stdout.
fn drawn(child: &mut Child) -> Receiver<Vec<u8>> {
    let mut stdout = child.stdout.take().unwrap();
    let (tell, drawn) = channel();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = stdout.read(&mut buffer) {
            if read == 0 || tell.send(buffer[..read].to_vec()).is_err() {
                break;
            }
        }
    });
    drawn
}

fn until_drawn(drawn: &Receiver<Vec<u8>>, wanted: &str) {
    let mut seen = Vec::new();
    loop {
        let Ok(bytes) = drawn.recv_timeout(PATIENCE) else {
            let tail = &seen[seen.len().saturating_sub(2000)..];
            panic!(
                "{wanted:?} never drew; the last it drew was {:?}",
                String::from_utf8_lossy(tail)
            );
        };
        // Only what could hold a new match is searched: a pane that drew megabytes first
        // would otherwise be searched from the start on every read.
        let from = seen.len().saturating_sub(wanted.len());
        seen.extend(bytes);
        if String::from_utf8_lossy(&seen[from..]).contains(wanted) {
            return;
        }
    }
}

/// A bridge says it attached, that it painted, and - when another bridge takes its pane -
/// that it was taken over, which is the one ending the window must not answer with another.
#[test]
fn a_bridge_says_it_attached_painted_and_was_taken_over() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));

    let first = Window::bind(&daemon, "first");
    let mut one = bridge(&daemon, "p1", &first, false);
    assert_eq!(first.next(), Report::Attached);
    assert!(matches!(first.next(), Report::Painted { .. }), "the replay painted something");

    let second = Window::bind(&daemon, "second");
    let mut two = bridge(&daemon, "p1", &second, true);
    assert_eq!(second.next(), Report::Attached);
    let ending = loop {
        match first.next() {
            Report::Painted { .. } => {}
            report => break exiting(report),
        }
    };
    assert_eq!(ending, Ending::TakenOver);
    assert!(one.wait().unwrap().success());
    let _ = two.kill();
    let _ = two.wait();
}

/// An attach the daemon refuses says why, so the window can tell a pane somebody else is
/// drawing from one that has gone.
#[test]
fn a_refused_attach_says_whether_the_pane_is_held_or_gone() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));

    let holder = Window::bind(&daemon, "holder");
    let mut holding = bridge(&daemon, "p1", &holder, false);
    assert_eq!(holder.next(), Report::Attached);

    let late = Window::bind(&daemon, "late");
    let mut refused = bridge(&daemon, "p1", &late, false);
    assert_eq!(exiting(late.next()), Ending::Refused);
    let _ = refused.wait();

    let nowhere = Window::bind(&daemon, "nowhere");
    let mut gone = bridge(&daemon, "p9", &nowhere, false);
    assert_eq!(exiting(nowhere.next()), Ending::Gone);
    let _ = gone.wait();
    let _ = holding.kill();
    let _ = holding.wait();
}

/// A keystroke goes to the daemon on the window's input connection, and what the program
/// echoes comes back through the bridge: the whole input path, with no bridge in the input.
#[test]
fn a_keystroke_reaches_the_program_and_its_echo_draws() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let window = Window::bind(&daemon, "window");
    let mut child = bridge(&daemon, "p1", &window, false);
    assert_eq!(window.next(), Report::Attached);
    let drawn = drawn(&mut child);

    let input = Input::open(daemon.socket_path(), "test", Box::new(|| {})).unwrap();
    input
        .send(proto::InputEvent {
            pane: "p1".to_string(),
            input: Some(input_event::Input::Send(input_event::Send {
                text: "echo bridged-$((6*7))".to_string(),
                enter: true,
                ..Default::default()
            })),
        })
        .unwrap();
    until_drawn(&drawn, "bridged-42");
    let _ = child.kill();
}

/// A daemon replaced by a new one hands its panes over on the same socket, and a bridge told
/// so attaches there and goes on drawing, rather than leaving the window to notice a dead pane
/// and start another.
#[test]
fn a_bridge_follows_its_pane_to_the_daemon_that_replaced_its_own() {
    let mut daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let window = Window::bind(&daemon, "window");
    let mut child = bridge(&daemon, "p1", &window, false);
    assert_eq!(window.next(), Report::Attached);
    let drawn = drawn(&mut child);
    drop(control);

    assert_eq!(daemon.replace(None).outcome(), proto::Outcome::Done);
    loop {
        match window.next() {
            Report::Attached => break,
            Report::Painted { .. } => {}
            other @ Report::Exiting(_) => {
                panic!("the bridge should attach again, and said {other:?}")
            }
        }
    }

    let input = Input::open(daemon.socket_path(), "test", Box::new(|| {})).unwrap();
    input
        .send(proto::InputEvent {
            pane: "p1".to_string(),
            input: Some(input_event::Input::Send(input_event::Send {
                text: "echo handed-$((6*7))".to_string(),
                enter: true,
                ..Default::default()
            })),
        })
        .unwrap();
    until_drawn(&drawn, "handed-42");
    assert!(child.try_wait().unwrap().is_none(), "the bridge is still drawing");
    let _ = child.kill();
}

/// A daemon hangs up on a bridge that has stopped reading its stream, and a bridge stops when
/// the surface it draws into stops reading it - which every surface in a window does while the
/// window's main thread is paging. Nothing is wrong with the bridge or its surface then, so it
/// attaches again and goes on drawing, where exiting would cost the window a new surface.
///
/// Once. A stream lost again straight after is not a window catching up, so the bridge exits
/// as it always did and the window's replacement policy decides.
#[test]
fn a_bridge_cut_off_for_not_reading_attaches_again_once() {
    let daemon = Daemon::start_with(built_daemon(), &[("MUSTER_DAEMON_STALLED_WRITE_MS", "300")]);
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let window = Window::bind(&daemon, "window");
    let mut child = bridge(&daemon, "p1", &window, false);
    assert_eq!(window.next(), Report::Attached);
    let reading = Arc::new(AtomicBool::new(false));
    let drawn = drawn_while(&mut child, Arc::clone(&reading));
    let input = Input::open(daemon.socket_path(), "test", Box::new(|| {})).unwrap();

    cut_off(&daemon, &input, &reading, 1);
    loop {
        match window.next() {
            Report::Attached => break,
            Report::Painted { .. } => {}
            other @ Report::Exiting(_) => {
                panic!("the bridge should attach again, and said {other:?}")
            }
        }
    }
    type_line(&input, "echo again-$((1+1))");
    until_drawn(&drawn, "again-2");
    assert!(child.try_wait().unwrap().is_none(), "the bridge is still drawing");

    cut_off(&daemon, &input, &reading, 2);
    let ending = loop {
        match window.next() {
            Report::Painted { .. } => {}
            report => break exiting(report),
        }
    };
    assert_eq!(ending, Ending::Lost);
    let _ = child.wait();
}

/// Stops reading what the bridge draws, floods the pane until the daemon hangs up on the
/// bridge for the `detached`th time, and reads again.
fn cut_off(daemon: &Daemon, input: &Input, reading: &AtomicBool, detached: usize) {
    reading.store(false, Ordering::SeqCst);
    type_line(input, "head -c 3000000 /dev/zero | base64");
    let log = daemon.socket_path().with_extension("log");
    let count = || {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .matches("\"event\":\"daemon.stream.detached\"")
            .count()
    };
    until(
        "the daemon to hang up on a bridge that stopped reading",
        || count() >= detached,
        || {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            let tail: String = text.lines().rev().take(12).collect::<Vec<_>>().join("\n");
            format!("{} hang-ups in {}; it ends:\n{tail}", count(), log.display())
        },
    );
    reading.store(true, Ordering::SeqCst);
}

fn type_line(input: &Input, text: &str) {
    input
        .send(proto::InputEvent {
            pane: "p1".to_string(),
            input: Some(input_event::Input::Send(input_event::Send {
                text: text.to_string(),
                enter: true,
                ..Default::default()
            })),
        })
        .unwrap();
}

/// What the bridge draws, read only while `reading` says so: a surface that has stopped
/// reading its pty.
fn drawn_while(child: &mut Child, reading: Arc<AtomicBool>) -> Receiver<Vec<u8>> {
    let mut stdout = child.stdout.take().unwrap();
    let (tell, drawn) = channel();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            if !reading.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            match stdout.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if tell.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    drawn
}
