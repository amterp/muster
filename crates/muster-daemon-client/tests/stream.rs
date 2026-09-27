//! The stream client a bridge draws a pane with, against the real daemon: it writes what the
//! daemon sends, acknowledges output, and ends when the pane is no longer its own.

use std::io::Write;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use muster_daemon_client::stream::{AttachError, Attachment, Ended, Happened};
use muster_daemon_proto as proto;
use muster_harness::requests::*;
use muster_harness::{Daemon, until, until_some};

const GRID: proto::Grid = proto::Grid { cols: 80, rows: 24, width_px: 800, height_px: 480 };

fn running(name: &str, tab: &str, command: &str) -> proto::pane_request::Create {
    proto::pane_request::Create {
        command: Some(command.to_string()),
        grid: Some(GRID),
        ..create(name, in_new_tab(tab))
    }
}

/// What a surface was sent, and a gate a test can close to make the surface stop reading.
#[derive(Clone, Default)]
struct Surface {
    written: Arc<Mutex<Vec<u8>>>,
    closed: Arc<(Mutex<bool>, Condvar)>,
    /// When the surface was last written to, and the longest it has gone between two writes.
    pace: Arc<Mutex<(Option<Instant>, Duration)>>,
}

impl Surface {
    fn stalled() -> Surface {
        let surface = Surface::default();
        *surface.closed.0.lock().unwrap() = true;
        surface
    }

    fn open(&self) {
        *self.closed.0.lock().unwrap() = false;
        self.closed.1.notify_all();
    }

    fn screen(&self) -> String {
        // Copied out first, so the surface is never kept waiting on a parse.
        let written = self.written.lock().unwrap().clone();
        let mut terminal = muster_vt::Terminal::new(80, 24).unwrap();
        terminal.write(&written);
        terminal.viewport(80, 24).render()
    }

    fn shows(&self, text: &str) -> bool {
        self.screen().contains(text)
    }

    /// Whether `text` is among the last bytes written, without parsing all of them.
    fn ends_with(&self, text: &str) -> bool {
        let written = self.written.lock().unwrap();
        let tail = &written[written.len().saturating_sub(4096)..];
        String::from_utf8_lossy(tail).contains(text)
    }

    fn longest_gap(&self) -> Duration {
        self.pace.lock().unwrap().1
    }
}

impl Write for Surface {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let (closed, opened) = &*self.closed;
        drop(opened.wait_while(closed.lock().unwrap(), |closed| *closed).unwrap());
        self.written.lock().unwrap().extend_from_slice(bytes);
        let mut pace = self.pace.lock().unwrap();
        let now = Instant::now();
        if let Some(last) = pace.0 {
            pace.1 = pace.1.max(now - last);
        }
        pace.0 = Some(now);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Pumping {
    ended: JoinHandle<Ended>,
    /// What happened, and how many bytes the surface held when it was said.
    happened: Receiver<(Happened, usize)>,
}

fn pump(attachment: Attachment, surface: &Surface) -> Pumping {
    let (tell, happened) = channel();
    let mut writer = surface.clone();
    let written = Arc::clone(&surface.written);
    let ended = std::thread::spawn(move || {
        attachment.pump(&mut writer, move |happening| {
            let _ = tell.send((happening, written.lock().unwrap().len()));
        })
    });
    Pumping { ended, happened }
}

fn open(daemon: &Daemon, pane: &str, takeover: bool) -> Attachment {
    Attachment::open(daemon.socket_path(), pane, GRID, takeover, "test").unwrap().0
}

#[test]
fn a_replay_then_the_panes_output_reach_the_surface_until_the_pane_closes() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    let flag = daemon.root().join("go");
    let wait = format!("while [ ! -e {} ]; do sleep 0.02; done", flag.display());
    make(&mut control, running("p1", "t1", &format!("echo before; {wait}; echo after")));
    until_text(&mut control, "p1", "before");

    let surface = Surface::default();
    let pumping = pump(open(&daemon, "p1", false), &surface);
    until("the replay to show the screen", || surface.shows("before"), || surface.screen());
    std::fs::write(&flag, "").unwrap();
    until("output after the replay", || surface.shows("after"), || surface.screen());

    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    assert_eq!(pumping.ended.join().unwrap(), Ended::Detached(proto::DetachReason::Closed));
}

/// A burst far larger than the window reaches a surface that keeps up whole, because the pane's
/// program waits for the surface's credit rather than the surface missing output.
///
/// A surface that was itself held up for the daemon's grace (100 ms) has not kept up, and is
/// rightly put behind; on a loaded machine that happens to the test's own thread. Such a run
/// proves nothing either way, so it is tried again, and only a run where the surface never
/// paused that long is judged.
#[test]
fn a_burst_reaches_a_surface_that_keeps_up_whole() {
    const GRACE: Duration = Duration::from_millis(100);
    for _ in 0..3 {
        let (surface, happened, replayed) = burst();
        if surface.longest_gap() >= GRACE {
            continue;
        }
        assert_eq!(happened, [], "a surface that writes at once is never behind");
        let written = surface.written.lock().unwrap();
        // "y\n" reaches the terminal as "y\r\n".
        let ys = String::from_utf8_lossy(&written[replayed..]).matches('y').count();
        assert_eq!(ys, 1_500_000, "every line of the burst reached the surface");
        return;
    }
    panic!("the surface was held up past the grace in every run: the machine is too busy to judge");
}

/// Runs a 3 MB burst into a surface that writes at once. Returns the surface, what the pump
/// said happened, and how many of the surface's bytes were the replay.
fn burst() -> (Surface, Vec<Happened>, usize) {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    let flag = daemon.root().join("go");
    let burst = format!(
        "while [ ! -e {} ]; do sleep 0.02; done; yes | head -c 3000000; echo; echo flooded",
        flag.display()
    );
    make(&mut control, running("p1", "t1", &burst));

    let surface = Surface::default();
    let pumping = pump(open(&daemon, "p1", false), &surface);
    until("the replay", || !surface.written.lock().unwrap().is_empty(), || surface.screen());
    let replayed = surface.written.lock().unwrap().len();
    // Measured from the burst's first write, not from the replay.
    *surface.pace.lock().unwrap() = (None, Duration::ZERO);
    std::fs::write(&flag, "").unwrap();
    until("the burst to end on the surface", || surface.ends_with("flooded"), || surface.screen());
    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    pumping.ended.join().unwrap();
    let happened = pumping.happened.try_iter().map(|(what, _)| what).collect();
    (surface, happened, replayed)
}

#[test]
fn a_surface_that_stalls_falls_behind_once_and_is_caught_up_with_the_screen() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(
        &mut control,
        running(
            "p1",
            "t1",
            "yes | head -c 3000000; echo; echo flooded; while :; do sleep 0.2; echo tick; done",
        ),
    );
    let surface = Surface::stalled();
    let pumping = pump(open(&daemon, "p1", false), &surface);
    until_text(&mut control, "p1", "flooded");
    surface.open();

    let (behind, when_behind) = pumping.happened.recv().unwrap();
    assert_eq!(behind, Happened::Behind);
    let (Happened::CaughtUp(bytes), when_caught_up) = pumping.happened.recv().unwrap() else {
        panic!("falling behind is followed by a catch-up");
    };
    assert!(bytes < 1_000_000, "the catch-up carries the screen, not the history: {bytes}");
    assert_eq!(
        when_caught_up - when_behind,
        bytes,
        "a catch-up is reported once all of it is on the surface, and counts all of it"
    );
    until("the catch-up to show the screen", || surface.shows("flooded"), || surface.screen());
    let written = surface.written.lock().unwrap().len();
    until(
        "output to resume",
        || surface.written.lock().unwrap().len() > written,
        || surface.screen(),
    );

    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    pumping.ended.join().unwrap();
    assert_eq!(pumping.happened.try_iter().count(), 0, "told once");
}

#[test]
fn a_resize_reaches_the_panes_terminal() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    let out = daemon.root().join("size");
    let watch =
        format!("while :; do stty size > {0}.new; mv {0}.new {0}; sleep 0.05; done", out.display());
    make(&mut control, running("p1", "t1", &watch));

    let attachment = open(&daemon, "p1", false);
    let resizer = attachment.resizer();
    let pumping = pump(attachment, &Surface::default());
    resizer.resize(proto::Grid { cols: 100, rows: 30, width_px: 1000, height_px: 600 }).unwrap();
    until_some("the pane to see its new size", || {
        std::fs::read_to_string(&out).ok().filter(|size| size.trim() == "30 100")
    });

    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    pumping.ended.join().unwrap();
}

#[test]
fn a_takeover_ends_the_stream_it_displaced() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, running("p1", "t1", "cat"));

    let first = pump(open(&daemon, "p1", false), &Surface::default());
    assert!(
        matches!(
            Attachment::open(daemon.socket_path(), "p1", GRID, false, "test"),
            Err(AttachError::Refused(_))
        ),
        "a second attach without takeover is refused"
    );
    let second = pump(open(&daemon, "p1", true), &Surface::default());
    assert_eq!(first.ended.join().unwrap(), Ended::Detached(proto::DetachReason::TakenOver));

    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    assert_eq!(second.ended.join().unwrap(), Ended::Detached(proto::DetachReason::Closed));
}

#[test]
fn a_pane_that_is_not_there_is_refused() {
    let daemon = Daemon::start_built();
    let refused = Attachment::open(daemon.socket_path(), "nowhere", GRID, false, "test");
    let Err(AttachError::Refused(reason)) = refused else {
        panic!("an attach to no pane is refused, and got {refused:?}");
    };
    assert!(reason.contains("nowhere"), "{reason}");
}
