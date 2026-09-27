//! The stream client a bridge draws a pane with (`muster-daemon-client`), against the real
//! daemon: it writes what the daemon sends, acknowledges output, and ends when the pane is no
//! longer its own.

mod support;

use std::io::Write;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use muster_daemon_client::stream::{AttachError, Attachment, Ended, Happened};
use support::*;

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
}

impl Write for Surface {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let (closed, opened) = &*self.closed;
        drop(opened.wait_while(closed.lock().unwrap(), |closed| *closed).unwrap());
        self.written.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Pumping {
    ended: JoinHandle<Ended>,
    happened: Receiver<Happened>,
}

fn pump(attachment: Attachment, surface: &Surface) -> Pumping {
    let (tell, happened) = channel();
    let mut surface = surface.clone();
    let ended = std::thread::spawn(move || {
        attachment.pump(&mut surface, move |happening| {
            let _ = tell.send(happening);
        })
    });
    Pumping { ended, happened }
}

fn open(daemon: &Daemon, pane: &str, takeover: bool) -> Attachment {
    Attachment::open(daemon.socket_path(), pane, GRID, takeover, "test").unwrap().0
}

#[test]
fn a_replay_then_the_panes_output_reach_the_surface_until_the_pane_closes() {
    let daemon = daemon();
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

#[test]
fn a_surface_that_keeps_up_is_never_behind() {
    let daemon = daemon();
    let mut control = daemon.connect();
    // Two megabytes, in bursts smaller than the window: without credit the surface would be
    // behind after the first quarter of a megabyte.
    let paced = "for i in $(seq 20); do yes | head -c 100000; sleep 0.05; done; echo; echo flooded";
    make(&mut control, running("p1", "t1", paced));

    let surface = Surface::default();
    let pumping = pump(open(&daemon, "p1", false), &surface);
    until("the flood to end on the surface", || surface.shows("flooded"), || surface.screen());
    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    pumping.ended.join().unwrap();
    let happened: Vec<Happened> = pumping.happened.try_iter().collect();
    assert_eq!(happened, [], "a surface that writes at once acknowledges in time");
}

#[test]
fn a_surface_that_stalls_falls_behind_once_and_is_caught_up_with_the_screen() {
    let daemon = daemon();
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

    assert_eq!(pumping.happened.recv().unwrap(), Happened::Behind);
    let Happened::CaughtUp(bytes) = pumping.happened.recv().unwrap() else {
        panic!("falling behind is followed by a catch-up");
    };
    assert!(bytes < 1_000_000, "the catch-up carries the screen, not the history: {bytes}");
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
    let daemon = daemon();
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
    let daemon = daemon();
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
    let daemon = daemon();
    let refused = Attachment::open(daemon.socket_path(), "nowhere", GRID, false, "test");
    let Err(AttachError::Refused(reason)) = refused else {
        panic!("an attach to no pane is refused, and got {refused:?}");
    };
    assert!(reason.contains("nowhere"), "{reason}");
}
