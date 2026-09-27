//! A stream connection: one pane's bytes, for the one bridge drawing it.
//!
//! A bridge attaches and gets a replay composed from the pane's terminal, then every chunk the
//! program writes, sent by the pane's reader before the terminal parses it. Frames go to a queue
//! drained by a writer thread of the stream's own, and credit bounds that queue: when a window of
//! output is unacknowledged, the reader waits for credit for a grace period, so a burst reaches a
//! bridge that keeps up whole. A bridge still short of room after the grace falls behind, stops
//! receiving output, and is caught up with the screen once its acknowledgements free half its
//! window (MIP-3 section 4).

use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection;
use muster_daemon_proto::{self as proto, stream_message, stream_request};
use prost::Message;

use crate::pane::PaneIo;
use crate::session::{self, Shared};

/// Output a bridge may have unacknowledged before the pane's program waits for it, unless the
/// bridge asks for another window when it attaches.
pub(crate) const WINDOW: u64 = 256 * 1024;

/// The windows a bridge may ask for. At least one read, so the window is not overshot many times
/// over; at most enough to fill a long, fast link, since every byte of it can be queued for the
/// bridge at once.
const WINDOWS: std::ops::RangeInclusive<u64> = 64 * 1024..=4 * 1024 * 1024;

/// How long a pane's reader waits, on each read, for its bridge to make room in a full window
/// before the bridge counts as behind.
///
/// Per read, so that a program is held to its bridge's pace for as long as the bridge keeps
/// crediting, as Ghostty holds a program to its parser and ssh to its channel window: a bridge
/// that keeps up gets every byte, however long the output. Per episode was tried and rejected,
/// because a bridge slower than its program - a local surface parsing a large file - then used
/// up its grace partway through and lost the middle. The cost is that a far bridge holds its
/// program to about a window per round trip, which is why a bridge may ask for a larger window;
/// and a bridge whose credit takes longer than this to come back stalls its program this long,
/// then falls behind, once per window (MIP-3 section 4).
pub(crate) const GRACE: Duration = Duration::from_millis(100);

/// The window a bridge asked for, within what the daemon allows.
pub(crate) fn window(asked: Option<u64>) -> u64 {
    match asked {
        None | Some(0) => WINDOW,
        Some(asked) => asked.clamp(*WINDOWS.start(), *WINDOWS.end()),
    }
}

/// The most replay one message carries. A replay spans the pane's whole history, which can be
/// larger than a frame may be, and the bridge writes the pieces to its surface in order.
const REPLAY_PIECE: usize = 1 << 20;

static NEXT_BRIDGE: AtomicU64 = AtomicU64::new(1);

/// How much output a bridge has not yet acknowledged, and whether it has fallen behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Credit {
    window: u64,
    unacknowledged: u64,
    behind: bool,
}

/// What to do with a chunk of output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Offer {
    Send,
    /// The window is full: tell the bridge, then send nothing until it catches up.
    FallBehind,
    Skip,
}

impl Credit {
    pub(crate) fn new(window: u64) -> Credit {
        Credit { window, unacknowledged: 0, behind: false }
    }

    /// Whether the window is full and the bridge is not yet behind: the pane's reader waits for
    /// credit before offering more (`PaneIo::wait_for_room`).
    pub(crate) fn is_full(&self) -> bool {
        !self.behind && self.unacknowledged >= self.window
    }

    /// Whether a chunk of `length` bytes goes to the bridge. While there is room it does, even
    /// past the window's edge, so the window is overshot by at most one read. A full window here
    /// normally means the reader already waited out its grace; a reset (`PaneIo::reset`) offers
    /// without waiting, and puts a bridge whose window is full at that moment behind at once.
    pub(crate) fn offer(&mut self, length: usize) -> Offer {
        if self.behind {
            return Offer::Skip;
        }
        if self.unacknowledged >= self.window {
            self.behind = true;
            return Offer::FallBehind;
        }
        self.unacknowledged += length as u64;
        Offer::Send
    }

    /// Takes the bridge's word that it has written `bytes` more to its surface. True when a
    /// bridge that fell behind has half its window free again, which is when it is caught up.
    ///
    /// Half, so that a slow bridge is not flipped between behind and caught up at the window's
    /// edge, each flip a catch-up composed under the pane's lock. Not an empty window: a bridge
    /// may acknowledge in batches, and one that fell behind is sent nothing more to complete its
    /// last batch with, so waiting for every byte would leave it blank for good. A bridge that
    /// credits in batches of at most half its window always reaches half; one that batches more
    /// can be left behind for good, holding less than a batch it will never complete.
    pub(crate) fn acknowledge(&mut self, bytes: u64) -> bool {
        self.unacknowledged = self.unacknowledged.saturating_sub(bytes);
        if self.behind && self.unacknowledged <= self.window / 2 {
            self.behind = false;
            return true;
        }
        false
    }
}

/// The bridge attached to a pane: where its frames go, and its credit.
#[derive(Debug)]
pub(crate) struct Bridge {
    id: u64,
    frames: Sender<Vec<u8>>,
    credit: Credit,
    /// The connection, to end the thread reading it once the pane lets the bridge go. That
    /// thread holds the pane, and a bridge that has stopped sending would keep it forever.
    socket: UnixStream,
}

impl Bridge {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn refused(self, kind: proto::AttachRefusal, reason: String) -> Refusal {
        Refusal { frames: self.frames, kind, reason }
    }

    pub(crate) fn attached(&self, offset: u64) {
        self.send(stream_message::Message::Attached(stream_message::Attached { offset }));
    }

    /// Hands the pane's output on, as credit allows.
    pub(crate) fn offer(&mut self, chunk: &[u8]) {
        match self.credit.offer(chunk.len()) {
            Offer::Send => self.send(stream_message::Message::Output(chunk.to_vec())),
            Offer::FallBehind => {
                self.send(stream_message::Message::Behind(stream_message::Behind {}));
            }
            Offer::Skip => {}
        }
    }

    pub(crate) fn is_full(&self) -> bool {
        self.credit.is_full()
    }

    /// True when the acknowledgement caught the bridge up, and it is owed a catch-up.
    pub(crate) fn acknowledge(&mut self, bytes: u64) -> bool {
        self.credit.acknowledge(bytes)
    }

    /// Sends a composed replay or catch-up, in pieces a frame can carry.
    pub(crate) fn replay(&self, bytes: &[u8]) {
        for piece in bytes.chunks(REPLAY_PIECE) {
            self.send(stream_message::Message::Replay(piece.to_vec()));
        }
    }

    /// Tells the bridge why it is let go. Its reading half is shut, which lets go of the pane
    /// at once; the frames already queued, this one last, are still written.
    pub(crate) fn detach(self, reason: proto::DetachReason) {
        self.send(stream_message::Message::Detached(stream_message::Detached {
            reason: reason.into(),
        }));
        let _ = self.socket.shutdown(Shutdown::Read);
    }

    fn send(&self, message: stream_message::Message) {
        let _ = self.frames.send(proto::StreamMessage { message: Some(message) }.encode_to_vec());
    }
}

/// Serves a welcomed stream connection until its bridge or its pane goes.
pub(crate) fn serve(mut stream: UnixStream, shared: &Arc<Shared>) {
    let frames = match writer(&stream) {
        Ok(frames) => frames,
        Err(error) => {
            log::error(
                "daemon.stream.not_served",
                fields! {
                    "error" => error,
                    "impact" => "a bridge's stream was closed unanswered, so its pane is not drawn",
                    "check" => "whether the daemon is out of threads or descriptors",
                },
            );
            return;
        }
    };
    let refuse = |frames: Sender<Vec<u8>>, kind: proto::AttachRefusal, reason: String| {
        let refused =
            stream_message::Message::Refused(proto::StreamRefused { reason, kind: kind.into() });
        let _ = frames.send(proto::StreamMessage { message: Some(refused) }.encode_to_vec());
    };

    let attach = match connection::receive::<proto::StreamRequest>(&mut stream) {
        Ok(Some(proto::StreamRequest {
            request: Some(stream_request::Request::Attach(attach)),
        })) => attach,
        Ok(None) => return,
        Ok(Some(_)) => {
            refuse(
                frames,
                proto::AttachRefusal::Malformed,
                "a stream's first message attaches it to a pane".to_string(),
            );
            return;
        }
        Err(error) => {
            refuse(
                frames,
                proto::AttachRefusal::Malformed,
                format!("the stream's first message did not read: {error}"),
            );
            return;
        }
    };
    let Some(io) = shared.lock().pane_io(&attach.pane) else {
        refuse(
            frames,
            proto::AttachRefusal::NoPane,
            format!("no pane {} on this daemon", attach.pane),
        );
        return;
    };
    let grid = match attach.grid.map(session::grid).transpose() {
        Ok(grid) => grid,
        Err(reason) => {
            refuse(frames, proto::AttachRefusal::BadGrid, reason);
            return;
        }
    };
    let socket = match stream.try_clone() {
        Ok(socket) => socket,
        Err(error) => {
            refuse(
                frames,
                proto::AttachRefusal::Unavailable,
                format!("the daemon could not hold the stream: {error}"),
            );
            return;
        }
    };
    let id = NEXT_BRIDGE.fetch_add(1, Ordering::Relaxed);
    let bridge = Bridge { id, frames, credit: Credit::new(window(attach.window)), socket };
    if let Err(refusal) = io.attach(bridge, grid, attach.takeover) {
        refuse(refusal.frames, refusal.kind, refusal.reason);
        return;
    }
    log::info("daemon.stream.attached", fields! { "pane" => attach.pane, "bridge" => id });

    follow(&mut stream, &io, id, &attach.pane);
    // Dropping the bridge, if the pane still holds it, ends the writer, which hangs the
    // connection up once what is queued is written.
    io.detach(id);
    log::info("daemon.stream.detached", fields! { "pane" => attach.pane, "bridge" => id });
}

/// Reads a bridge's credit and resizes until it hangs up or is detached.
fn follow(stream: &mut UnixStream, io: &Arc<PaneIo>, id: u64, pane: &str) {
    loop {
        let request = match connection::receive::<proto::StreamRequest>(stream) {
            Ok(Some(request)) => request,
            Ok(None) => return,
            Err(error) => {
                log::warn(
                    "daemon.stream.unreadable",
                    fields! {
                        "pane" => pane,
                        "error" => error,
                        "impact" => "the stream is closed; its bridge has to attach again",
                        "check" => "whether the bridge speaks this daemon's protocol version",
                    },
                );
                return;
            }
        };
        match request.request {
            Some(stream_request::Request::Credit(credit)) => io.acknowledge(id, credit.bytes),
            Some(stream_request::Request::Resize(resize)) => match resize.grid.map(session::grid) {
                Some(Ok(grid)) => io.resize(grid),
                Some(Err(reason)) => log::warn(
                    "daemon.stream.bad_resize",
                    fields! {
                        "pane" => pane,
                        "reason" => reason,
                        "impact" => "the pane keeps the size it had",
                    },
                ),
                None => {}
            },
            Some(stream_request::Request::Attach(_)) | None => {}
        }
    }
}

/// How long a write to a bridge may make no progress before the bridge counts as gone. A slow
/// link still moves; a forward whose far end has stopped reading does not, and its queue
/// would otherwise be held until the connection died of something else.
const STALLED_WRITE: Duration = Duration::from_secs(30);

/// Starts the thread that writes a stream's frames, and returns where to queue them. When every
/// sender has gone, it writes what is left and hangs the connection up, which ends the thread
/// reading it.
fn writer(stream: &UnixStream) -> std::io::Result<Sender<Vec<u8>>> {
    let (frames, queued) = mpsc::channel::<Vec<u8>>();
    let mut writing = stream.try_clone()?;
    writing.set_write_timeout(Some(STALLED_WRITE))?;
    std::thread::Builder::new().name("stream write".to_string()).spawn(move || {
        for frame in queued {
            if muster_frame::write_frame(&mut writing, &frame).is_err() {
                break;
            }
        }
        let _ = writing.shutdown(Shutdown::Both);
    })?;
    Ok(frames)
}

#[cfg(test)]
impl Bridge {
    /// A bridge whose frames arrive on the receiver returned, over a socket nobody reads.
    pub(crate) fn for_test() -> (Bridge, mpsc::Receiver<Vec<u8>>) {
        let (frames, received) = mpsc::channel();
        let (socket, _) = UnixStream::pair().expect("a socket pair");
        let id = NEXT_BRIDGE.fetch_add(1, Ordering::Relaxed);
        (Bridge { id, frames, credit: Credit::new(WINDOW), socket }, received)
    }
}

/// Why a bridge was not attached, with its queue back so it can be told.
#[derive(Debug)]
pub(crate) struct Refusal {
    pub(crate) frames: Sender<Vec<u8>>,
    pub(crate) kind: proto::AttachRefusal,
    pub(crate) reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_sent_until_the_window_fills_then_skipped() {
        let mut credit = Credit::new(100);
        assert_eq!(credit.offer(60), Offer::Send);
        assert_eq!(credit.offer(60), Offer::Send, "room left, so the window may be overshot");
        assert_eq!(credit.offer(10), Offer::FallBehind);
        assert_eq!(credit.offer(10), Offer::Skip);
    }

    #[test]
    fn credit_makes_room_and_a_bridge_behind_is_caught_up_once_it_has_room() {
        let mut credit = Credit::new(100);
        credit.offer(60);
        assert!(!credit.acknowledge(60), "a bridge that never fell behind owes no catch-up");
        credit.offer(120);
        assert_eq!(credit.offer(1), Offer::FallBehind);
        assert!(!credit.acknowledge(20), "a hundred bytes still unacknowledged: a full window");
        assert_eq!(credit.offer(1), Offer::Skip);
        assert!(!credit.acknowledge(1), "room at the edge is not enough to catch up with");
        assert!(!credit.acknowledge(48), "fifty-one bytes: still more than half the window");
        assert!(credit.acknowledge(1), "half the window free");
        assert_eq!(credit.offer(1), Offer::Send, "caught up, output flows again");
    }

    #[test]
    fn a_bridge_that_acknowledges_in_batches_is_caught_up_all_the_same() {
        const BATCH: u64 = 32 * 1024;
        let mut credit = Credit::new(WINDOW);
        let mut sent = 0;
        // Reads come in whatever sizes the program wrote, rarely a batch's multiple.
        while credit.offer(20_000) == Offer::Send {
            sent += 20_000;
        }
        // The bridge writes everything it was sent and acknowledges each whole batch, holding
        // back the part of one it has not filled - and it is sent nothing more to fill it with.
        let mut caught_up = false;
        for _ in 0..sent / BATCH {
            caught_up |= credit.acknowledge(BATCH);
        }
        assert!(caught_up, "a bridge that acknowledged {} of {sent} bytes is still behind", {
            sent / BATCH * BATCH
        });
    }

    #[test]
    fn a_full_window_is_waited_on_until_the_bridge_is_behind() {
        let mut credit = Credit::new(100);
        credit.offer(99);
        assert!(!credit.is_full());
        credit.offer(1);
        assert!(credit.is_full(), "the reader waits for credit");
        credit.acknowledge(1);
        assert!(!credit.is_full());
        credit.offer(1);
        assert_eq!(credit.offer(1), Offer::FallBehind, "the grace ran out");
        assert!(!credit.is_full(), "a bridge behind is not waited on again");
    }

    #[test]
    fn a_bridge_gets_the_window_it_asks_for_within_bounds() {
        assert_eq!(window(None), WINDOW);
        assert_eq!(window(Some(0)), WINDOW, "zero is no answer");
        assert_eq!(window(Some(1 << 20)), 1 << 20);
        assert_eq!(window(Some(1)), 64 * 1024);
        assert_eq!(window(Some(u64::MAX)), 4 << 20);
    }

    #[test]
    fn acknowledging_more_than_was_sent_is_not_credit_for_later() {
        let mut credit = Credit::new(100);
        credit.acknowledge(1_000);
        credit.offer(99);
        assert_eq!(credit.offer(1), Offer::Send);
        assert_eq!(credit.offer(1), Offer::FallBehind);
    }
}
