//! Drawing a pane from muster-daemon rather than herdr (MIP-3, section 4).
//!
//! The daemon sends the program's own bytes, so this writes them to the surface as they come and
//! acknowledges each piece, which is all that keeps the daemon sending. The surface's writes -
//! key encodings, answers to queries - are read and dropped: the daemon is the only writer to a
//! pane, and an unread terminal would eventually stop the surface writing at all.

use std::io::Read;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_client::stream::{Attachment, Ended, Happened};
use muster_daemon_proto::Grid;

use crate::pty;

pub(crate) struct Arguments {
    /// Muster's name for the pane, which is also the daemon's.
    pane: String,
    socket: String,
    takeover: bool,
}

impl Arguments {
    pub(crate) fn parse(arguments: &[String]) -> Option<Arguments> {
        let mut read = arguments.iter();
        let pane = read.next().filter(|pane| !pane.starts_with('-'))?.clone();
        let (mut socket, mut takeover) = (None, false);
        while let Some(flag) = read.next() {
            match flag.as_str() {
                "--daemon-socket" => socket = Some(read.next()?.clone()),
                "--takeover" => takeover = true,
                _ => return None,
            }
        }
        Some(Arguments { pane, socket: socket?, takeover })
    }
}

/// Draws the pane until the daemon lets it go, then exits.
pub(crate) fn run(arguments: &Arguments) -> ! {
    log::start_from_environment(format!("bridge:{}", arguments.pane));
    // Before any thread exists, so they all inherit the blocked signal.
    let resizes = pty::watch_for_resize();
    pty::make_stdin_raw();
    std::thread::spawn(discard_stdin);

    let grid = surface_grid();
    log::info(
        "bridge.start",
        fields! {
            "pane" => arguments.pane,
            "daemon" => arguments.socket,
            "cols" => grid.cols,
            "rows" => grid.rows,
            "takeover" => arguments.takeover.to_string(),
        },
    );
    let opened = Attachment::open(
        arguments.socket.as_ref(),
        &arguments.pane,
        grid,
        arguments.takeover,
        &format!("muster-bridge {}", env!("CARGO_PKG_VERSION")),
    );
    let attachment = match opened {
        Ok((attachment, _)) => attachment,
        Err(error) => {
            log::error(
                "bridge.attach.failed",
                fields! {
                    "pane" => arguments.pane,
                    "daemon" => arguments.socket,
                    "error" => error,
                    "impact" => "this pane renders nothing",
                    "check" => "that a muster-daemon is listening on that socket and holds the \
                                pane, and that it speaks this bridge's protocol version",
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

    let daemon = attachment.resizer();
    std::thread::spawn(move || {
        for () in resizes {
            let grid = surface_grid();
            log::info("bridge.resize", fields! { "cols" => grid.cols, "rows" => grid.rows });
            // A daemon that has hung up ends the pump, which says so.
            let _ = daemon.resize(grid);
        }
    });

    let ended = attachment.pump(&mut std::io::stdout().lock(), |happened| match happened {
        Happened::Behind => log::info(
            "bridge.behind",
            fields! {
                "impact" => "output is skipped until this surface catches up; what scrolled \
                             past meanwhile is in the daemon, not in the surface's scrollback",
            },
        ),
        Happened::CaughtUp(bytes) => log::info("bridge.caught_up", fields! { "bytes" => bytes }),
    });
    match &ended {
        Ended::Detached(reason) => {
            log::info("bridge.detached", fields! { "reason" => reason.as_str_name() });
        }
        Ended::HungUp => log::info("bridge.hung_up", fields! {}),
        Ended::Failed(error) => log::warn(
            "bridge.stream.failed",
            fields! {
                "error" => error,
                "impact" => "this pane stops drawing; a new bridge attaches with a fresh replay",
            },
        ),
    }
    std::process::exit(exit_status(&ended));
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
        assert!(!parsed.takeover);
        assert!(parse(&["p1", "--takeover", "--daemon-socket", "/s"]).unwrap().takeover);
    }

    /// herdr's flags mean nothing to a daemon, and half a herdr command line is a mistake that
    /// would otherwise draw from the wrong place.
    #[test]
    fn nothing_of_herdrs_is_taken() {
        assert!(parse(&["p1", "--daemon-socket", "/s", "--herdr-socket", "/h"]).is_none());
        assert!(parse(&["p1", "--daemon-socket", "/s", "--control-socket", "/c"]).is_none());
        assert!(parse(&["p1", "--daemon-socket"]).is_none(), "a socket names a path");
        assert!(parse(&["--daemon-socket", "/s"]).is_none(), "a pane comes first");
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
}
