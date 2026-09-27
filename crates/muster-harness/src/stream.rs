//! A stream connection as a test drives one: attach to a pane, then see every message in the
//! order the daemon sent it. Credit is the test's to give, or to withhold.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use muster_daemon_proto::connection;
use muster_daemon_proto::{self as proto, ConnectionKind, stream_message, stream_request};

/// A welcomed stream connection.
#[derive(Debug)]
pub struct Stream {
    stream: UnixStream,
}

impl Stream {
    /// Connects and says hello, panicking if the daemon will not talk.
    pub fn connect(socket: &Path) -> Stream {
        let (stream, _) = connection::connect(socket, ConnectionKind::Stream, "harness")
            .unwrap_or_else(|error| {
                panic!("could not open a stream connection to {}: {error}", socket.display())
            });
        Stream { stream }
    }

    /// Asks for a pane's stream, at `grid` if one is given.
    pub fn attach(&mut self, pane: &str, grid: Option<proto::Grid>, takeover: bool) {
        self.send(stream_request::Request::Attach(stream_request::Attach {
            pane: pane.to_string(),
            grid,
            takeover,
        }));
    }

    /// Acknowledges output written to the surface.
    pub fn credit(&mut self, bytes: u64) {
        self.send(stream_request::Request::Credit(stream_request::Credit { bytes }));
    }

    pub fn resize(&mut self, grid: proto::Grid) {
        self.send(stream_request::Request::Resize(stream_request::Resize { grid: Some(grid) }));
    }

    /// The next message, `Some(None)` once the daemon hangs up, or `None` if nothing arrived
    /// within `within`.
    #[allow(clippy::option_option)] // three answers: a message, a hang-up, or nothing yet
    pub fn next_within(&mut self, within: Duration) -> Option<Option<stream_message::Message>> {
        // macOS refuses the option once the daemon has hung up; what it sent is still there to
        // read, and then the end.
        let _ = self.stream.set_read_timeout(Some(within));
        match connection::receive::<proto::StreamMessage>(&mut self.stream) {
            Ok(Some(message)) => Some(message.message),
            Err(error) if error.contains("timed out") || error.contains("temporarily") => None,
            Ok(None) | Err(_) => Some(None),
        }
    }

    /// A write the daemon no longer reads is what a hung-up stream looks like from here, as it
    /// does to a real bridge, so it is not a failure; the next read reports the end.
    fn send(&mut self, request: stream_request::Request) {
        let _ =
            connection::send(&mut self.stream, &proto::StreamRequest { request: Some(request) });
    }
}
