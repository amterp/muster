//! A stream connection: one pane's bytes, for the one bridge drawing it.
//!
//! A bridge attaches and gets a replay composed from the pane's terminal, then every chunk the
//! program writes, sent by the pane's reader before the terminal parses it. Nothing the bridge
//! does can make the reader wait: frames go to a queue drained by a writer thread of the
//! stream's own, and credit bounds that queue. A bridge that stops acknowledging what it has
//! written to its surface falls behind, stops receiving output, and is caught up with the
//! screen once it has acknowledged everything it was sent (MIP-3 section 4).

use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection;
use muster_daemon_proto::{self as proto, stream_message, stream_request};
use prost::Message;

use crate::pane::PaneIo;
use crate::session::{self, Shared};

/// Output a bridge may have unacknowledged before the pane counts as behind.
pub(crate) const WINDOW: u64 = 256 * 1024;

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

    /// Whether a chunk of `length` bytes goes to the bridge. While there is room it does, even
    /// past the window's edge, so the window is overshot by at most one read.
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

    /// Takes the bridge's word that it has written `bytes` more to its surface. True when that
    /// was everything sent to a bridge that fell behind, which is when it is caught up.
    pub(crate) fn acknowledge(&mut self, bytes: u64) -> bool {
        self.unacknowledged = self.unacknowledged.saturating_sub(bytes);
        if self.behind && self.unacknowledged == 0 {
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
}

impl Bridge {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn refused(self, reason: String) -> Refusal {
        Refusal { frames: self.frames, reason }
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

    pub(crate) fn detach(self, reason: proto::DetachReason) {
        self.send(stream_message::Message::Detached(stream_message::Detached {
            reason: reason.into(),
        }));
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
    let refuse = |frames: Sender<Vec<u8>>, reason: String| {
        let refused = stream_message::Message::Refused(proto::StreamRefused { reason });
        let _ = frames.send(proto::StreamMessage { message: Some(refused) }.encode_to_vec());
    };

    let attach = match connection::receive::<proto::StreamRequest>(&mut stream) {
        Ok(Some(proto::StreamRequest {
            request: Some(stream_request::Request::Attach(attach)),
        })) => attach,
        Ok(None) => return,
        Ok(Some(_)) => {
            refuse(frames, "a stream's first message attaches it to a pane".to_string());
            return;
        }
        Err(error) => {
            refuse(frames, format!("the stream's first message did not read: {error}"));
            return;
        }
    };
    let Some(io) = shared.lock().pane_io(&attach.pane) else {
        refuse(frames, format!("no pane {} on this daemon", attach.pane));
        return;
    };
    let grid = match attach.grid.map(session::grid).transpose() {
        Ok(grid) => grid,
        Err(reason) => {
            refuse(frames, reason);
            return;
        }
    };
    let id = NEXT_BRIDGE.fetch_add(1, Ordering::Relaxed);
    let bridge = Bridge { id, frames, credit: Credit::new(WINDOW) };
    if let Err(refusal) = io.attach(bridge, grid, attach.takeover) {
        refuse(refusal.frames, refusal.reason);
        return;
    }
    log::info("daemon.stream.attached", fields! { "pane" => attach.pane, "bridge" => id });

    follow(&mut stream, &io, id, &attach.pane);
    io.detach(id);
    let _ = stream.shutdown(Shutdown::Both);
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

/// Starts the thread that writes a stream's frames, and returns where to queue them. When every
/// sender has gone, it writes what is left and hangs the connection up, which ends the thread
/// reading it.
fn writer(stream: &UnixStream) -> std::io::Result<Sender<Vec<u8>>> {
    let (frames, queued) = mpsc::channel::<Vec<u8>>();
    let mut writing = stream.try_clone()?;
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

/// Why a bridge was not attached, with its queue back so it can be told.
#[derive(Debug)]
pub(crate) struct Refusal {
    pub(crate) frames: Sender<Vec<u8>>,
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
    fn credit_makes_room_and_a_bridge_behind_is_caught_up_only_when_acknowledged_in_full() {
        let mut credit = Credit::new(100);
        credit.offer(60);
        assert!(!credit.acknowledge(60), "a bridge that never fell behind owes no catch-up");
        credit.offer(120);
        assert_eq!(credit.offer(1), Offer::FallBehind);
        assert!(!credit.acknowledge(100), "twenty bytes still unacknowledged");
        assert_eq!(credit.offer(1), Offer::Skip);
        assert!(credit.acknowledge(20));
        assert_eq!(credit.offer(1), Offer::Send, "caught up, output flows again");
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
