//! Drawing a pane from its daemon (MIP-3, section 4).
//!
//! The daemon sends the program's own bytes, so this writes them to the surface as they come and
//! acknowledges each piece, which is all that keeps the daemon sending. The surface's writes -
//! key encodings, answers to queries - are read and dropped: the daemon is the only writer to a
//! pane, and an unread terminal would eventually stop the surface writing at all.

use std::io::Read;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use muster_core::bridge_link::Report;
use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::respawn::{Ended as Exit, Ending};
use muster_daemon_client::stream::{AttachError, Attachment, Ended, Happened, Resizer};
use muster_daemon_proto::{AttachRefusal, DetachReason, Grid};

use crate::link::{CountedSurface, Counting, Link};
use crate::pty;

/// The window of unacknowledged output a bridge asks for across an ssh forward: ssh's own
/// channel window, measured as what keeps a remote program from being held to one small window
/// per round trip (MIP-3, section 4).
const REMOTE_WINDOW: u64 = 2 << 20;

pub(crate) struct Arguments {
    /// Muster's name for the pane, which is also the daemon's.
    pane: String,
    socket: String,
    app_socket: Option<String>,
    remote: bool,
    takeover: bool,
}

impl Arguments {
    pub(crate) fn parse(arguments: &[String]) -> Option<Arguments> {
        let mut read = arguments.iter();
        let pane = read.next().filter(|pane| !pane.starts_with('-'))?.clone();
        let (mut socket, mut app_socket, mut remote, mut takeover) = (None, None, false, false);
        while let Some(flag) = read.next() {
            match flag.as_str() {
                "--daemon-socket" => socket = Some(read.next()?.clone()),
                "--app-socket" => app_socket = Some(read.next()?.clone()),
                "--remote" => remote = true,
                "--takeover" => takeover = true,
                _ => return None,
            }
        }
        Some(Arguments { pane, socket: socket?, app_socket, remote, takeover })
    }
}

/// Draws the pane until the daemon lets it go, then exits.
pub(crate) fn run(arguments: &Arguments) -> ! {
    log::start_from_environment(format!("bridge:{}", arguments.pane));
    // Before any thread exists, so they all inherit the blocked signal.
    let resizes = pty::watch_for_resize();
    pty::make_stdin_raw();
    std::thread::spawn(discard_stdin);

    log::info(
        "bridge.start",
        fields! {
            "pane" => arguments.pane,
            "daemon" => arguments.socket,
            "takeover" => arguments.takeover.to_string(),
        },
    );
    // Before attaching, so a bridge the daemon refuses can still say why.
    let link = Link::dial(arguments.app_socket.as_deref());
    let counting = Arc::new(Counting::new());
    {
        let (counting, link) = (Arc::clone(&counting), link.clone());
        std::thread::spawn(move || counting.report(&link));
    }
    // Whichever attachment is current, since a replaced daemon's pane is attached again.
    let current: Arc<Mutex<Option<Resizer>>> = Arc::default();
    {
        let current = Arc::clone(&current);
        std::thread::spawn(move || {
            for () in resizes {
                let grid = surface_grid();
                log::info("bridge.resize", fields! { "cols" => grid.cols, "rows" => grid.rows });
                if let Some(resizer) =
                    current.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
                {
                    // A daemon that has hung up ends the pump, which says so.
                    let _ = resizer.resize(grid);
                }
            }
        });
    }
    let mut surface = CountedSurface { surface: std::io::stdout().lock(), counting: &counting };

    let mut attached = attach(arguments, arguments.takeover);
    loop {
        let attachment = match attached {
            Ok(attachment) => attachment,
            Err(error) => {
                link.say(&Report::Exiting(refusal(&error)));
                log::error(
                    "bridge.attach.failed",
                    fields! {
                        "pane" => arguments.pane,
                        "daemon" => arguments.socket,
                        "error" => error,
                        "impact" => "this pane renders nothing",
                        "check" => "that a muster-daemon is listening on that socket and holds \
                                    the pane, and that it speaks this bridge's protocol version",
                    },
                );
                eprint!(
                    "muster-bridge: could not attach to pane {} on {}: {error}\n\
                     This pane will render nothing.\n\n",
                    arguments.pane, arguments.socket
                );
                std::process::exit(1);
            }
        };
        link.say(&Report::Attached);
        *current.lock().unwrap_or_else(PoisonError::into_inner) = Some(attachment.resizer());

        let ended = attachment.pump(&mut surface, |happened| match happened {
            Happened::Behind => log::info(
                "bridge.behind",
                fields! {
                    "impact" => "output is skipped until this surface catches up; what scrolled \
                                 past meanwhile is in the daemon, not in the surface's scrollback",
                },
            ),
            Happened::CaughtUp(bytes) => {
                log::info("bridge.caught_up", fields! { "bytes" => bytes });
            }
        });
        match &ended {
            Ended::Detached(DetachReason::Replaced) => {
                // The pane went to the daemon replacing this one, on the same socket, whose
                // replay draws the same screen.
                log::info("bridge.replaced", fields! { "pane" => arguments.pane });
                attached = attach_again(arguments);
                continue;
            }
            Ended::Detached(reason) => {
                log::info("bridge.detached", fields! { "reason" => reason.as_str_name() });
            }
            Ended::HungUp => log::info("bridge.hung_up", fields! {}),
            Ended::Failed(error) => log::warn(
                "bridge.stream.failed",
                fields! {
                    "error" => error,
                    "impact" => "this pane stops drawing; a new bridge attaches with a fresh \
                                 replay",
                },
            ),
        }
        link.say(&Report::Exiting(exit(&ended, counting.painted())));
        std::process::exit(exit_status(&ended));
    }
}

/// How long a replaced daemon's successor has to start taking attaches.
const REPLACED_WITHIN: Duration = Duration::from_secs(10);

fn attach(arguments: &Arguments, takeover: bool) -> Result<Attachment, AttachError> {
    let grid = surface_grid();
    log::info("bridge.attach", fields! { "cols" => grid.cols, "rows" => grid.rows });
    Attachment::open_with_window(
        arguments.socket.as_ref(),
        &arguments.pane,
        grid,
        takeover,
        arguments.remote.then_some(REMOTE_WINDOW),
        &format!("muster-bridge {}", env!("CARGO_PKG_VERSION")),
    )
    .map(|(attachment, _)| attachment)
}

/// Attaches to the daemon that replaced the one this bridge was drawing from.
///
/// Retried while the socket answers nothing or hangs up, since the successor may still be
/// taking it over; a refusal is an answer, and is not retried. Never a takeover: the daemon
/// before let this bridge go, so nothing else should be drawing the pane.
fn attach_again(arguments: &Arguments) -> Result<Attachment, AttachError> {
    let deadline = Instant::now() + REPLACED_WITHIN;
    let mut pause = Duration::from_millis(20);
    loop {
        match attach(arguments, false) {
            Err(AttachError::Handshake(_) | AttachError::Broken(_))
                if Instant::now() < deadline =>
            {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(500));
            }
            attached => return attached,
        }
    }
}

/// What the window is told about a stream that ended: whether to start another bridge.
fn exit(ended: &Ended, rendered: bool) -> Exit {
    let (ending, reason) = match ended {
        Ended::Detached(DetachReason::TakenOver) => {
            (Ending::TakenOver, "another bridge attached to this pane".to_string())
        }
        Ended::Detached(DetachReason::Closed | DetachReason::Exited) => {
            (Ending::Gone, "the pane closed".to_string())
        }
        Ended::Detached(reason) => {
            (Ending::Lost, format!("the daemon let the pane go ({})", reason.as_str_name()))
        }
        Ended::HungUp => (Ending::Lost, "the daemon hung up".to_string()),
        Ended::Failed(error) => (Ending::Lost, error.clone()),
    };
    Exit { ending, reason: Some(reason), rendered }
}

/// What the window is told about an attach the daemon refused.
///
/// Only a pane the daemon no longer holds is gone, and only another bridge drawing it is a
/// refusal the window should respect. Every other refusal says nothing about the pane, so the
/// window may start another bridge for it.
fn refusal(error: &AttachError) -> Exit {
    let ending = match error {
        AttachError::Refused {
            kind: AttachRefusal::NoPane | AttachRefusal::PaneClosed, ..
        } => Ending::Gone,
        AttachError::Refused { kind: AttachRefusal::AttachedElsewhere, .. } => Ending::Refused,
        AttachError::Refused { .. } | AttachError::Handshake(_) | AttachError::Broken(_) => {
            Ending::Lost
        }
    };
    Exit { ending, reason: Some(error.to_string()), rendered: false }
}

/// A stream that broke is a failure, like an attach that never happened. The daemon letting
/// the pane go, for any reason, and hanging up are how a bridge is meant to end.
fn exit_status(ended: &Ended) -> i32 {
    match ended {
        Ended::Detached(_) | Ended::HungUp => 0,
        Ended::Failed(_) => 1,
    }
}

/// The surface's grid, from the PTY libghostty sized for it.
fn surface_grid() -> Grid {
    let size = pty::window_size();
    Grid {
        cols: size.ws_col.into(),
        rows: size.ws_row.into(),
        width_px: size.ws_xpixel.into(),
        height_px: size.ws_ypixel.into(),
    }
}

fn discard_stdin() {
    let mut buffer = [0u8; 4096];
    let mut discarded = 0u64;
    let mut stdin = std::io::stdin().lock();
    while let Ok(read) = stdin.read(&mut buffer) {
        if read == 0 {
            break;
        }
        discarded += read as u64;
    }
    log::debug("bridge.stdin.closed", fields! { "discarded" => discarded });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Option<Arguments> {
        Arguments::parse(&words.iter().map(ToString::to_string).collect::<Vec<_>>())
    }

    #[test]
    fn a_pane_and_a_socket_are_all_it_needs() {
        let parsed = parse(&["p1w3r07bsd", "--daemon-socket", "/tmp/d.sock"]).unwrap();
        assert_eq!((parsed.pane.as_str(), parsed.socket.as_str()), ("p1w3r07bsd", "/tmp/d.sock"));
        assert!(!parsed.takeover && !parsed.remote && parsed.app_socket.is_none());
        let every =
            parse(&["p1", "--takeover", "--daemon-socket", "/s", "--app-socket", "/a", "--remote"])
                .unwrap();
        assert!(every.takeover && every.remote);
        assert_eq!(every.app_socket.as_deref(), Some("/a"));
    }

    #[test]
    fn a_flag_it_does_not_know_is_refused() {
        assert!(parse(&["p1", "--daemon-socket", "/s", "--herdr-socket", "/h"]).is_none());
        assert!(parse(&["p1", "--daemon-socket"]).is_none(), "a socket names a path");
        assert!(parse(&["p1", "--daemon-socket", "/s", "--app-socket"]).is_none());
        assert!(parse(&["--daemon-socket", "/s"]).is_none(), "a pane comes first");
    }

    /// Only a bridge the pane was taken from, or whose pane is gone, is not replaced; every
    /// other ending asks the window to look again.
    #[test]
    fn the_window_hears_whether_to_start_another() {
        let ending = |ended: Ended| exit(&ended, true).ending;
        assert_eq!(ending(Ended::Detached(DetachReason::TakenOver)), Ending::TakenOver);
        assert_eq!(ending(Ended::Detached(DetachReason::Closed)), Ending::Gone);
        assert_eq!(ending(Ended::Detached(DetachReason::Exited)), Ending::Gone);
        assert_eq!(ending(Ended::HungUp), Ending::Lost);
        assert_eq!(ending(Ended::Failed("reset".into())), Ending::Lost);
    }

    #[test]
    fn only_a_broken_stream_exits_as_a_failure() {
        use muster_daemon_proto::DetachReason;
        assert_eq!(exit_status(&Ended::Failed("reading p1's stream: reset".into())), 1);
        assert_eq!(exit_status(&Ended::HungUp), 0);
        for reason in [DetachReason::Closed, DetachReason::Exited, DetachReason::TakenOver] {
            assert_eq!(exit_status(&Ended::Detached(reason)), 0, "{reason:?}");
        }
    }

    /// Only a pane that is not there is gone, and only another bridge drawing it is to be
    /// respected; any other refusal leaves the window free to try again.
    #[test]
    fn a_refusal_is_gone_only_when_the_pane_is() {
        let refused = |kind| AttachError::Refused { kind, reason: String::new() };
        let cases = [
            (AttachRefusal::NoPane, Ending::Gone),
            (AttachRefusal::PaneClosed, Ending::Gone),
            (AttachRefusal::AttachedElsewhere, Ending::Refused),
            (AttachRefusal::BadGrid, Ending::Lost),
            (AttachRefusal::Malformed, Ending::Lost),
            (AttachRefusal::Unavailable, Ending::Lost),
            (AttachRefusal::Unspecified, Ending::Lost),
        ];
        for (kind, ending) in cases {
            assert_eq!(refusal(&refused(kind)).ending, ending, "{kind:?}");
        }
    }
}
