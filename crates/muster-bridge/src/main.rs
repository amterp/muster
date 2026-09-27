//! muster-bridge: one pane's output, drawn into one surface.
//!
//! libghostty gives a surface no way to be fed bytes, so the only channel into it is the
//! command it spawns (docs/observations/libghostty-9f9b8d1d.md section 2). This is that
//! command. It attaches to the pane's stream on the daemon that holds it and writes the
//! program's own bytes to its stdout, which is the surface's PTY (MIP-3, section 4).
//!
//! Output only. Input goes from the window to the daemon on a connection of its own, and the
//! daemon is the only writer to a pane; what the surface writes back is read and dropped.
//!
//! It also tells the window about itself, on a socket the window binds for the pane
//! (`muster_core::bridge_link`): that it attached, that it is painting, and why it exits.

mod daemon;
mod link;
mod pty;
mod tally;

const USAGE: &str = "\
usage: muster-bridge <pane> --daemon-socket <path> [--app-socket <path>] [--remote] [--takeover]

  <pane>             Muster's name for the pane, which the daemon knows it by
  --daemon-socket    the socket of the daemon holding the pane: its own, or the local end of
                     an ssh forward to it
  --app-socket       the socket the window bound for this pane, which hears that the bridge
                     attached, is painting, and why it exits
  --remote           the daemon is at the far end of an ssh forward, so the stream asks for a
                     window of unacknowledged output sized for the link
  --takeover         displace a bridge already drawing the pane, as a replacement for one that
                     died does";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(arguments) = daemon::Arguments::parse(&arguments) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    daemon::run(&arguments);
}
