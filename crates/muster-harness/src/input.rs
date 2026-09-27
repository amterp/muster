//! An input connection as a test drives one. Nothing comes back on it: what a test sees of
//! input is what the pane's program received.

use std::os::unix::net::UnixStream;
use std::path::Path;

use muster_daemon_proto::connection;
use muster_daemon_proto::{self as proto, ConnectionKind, input_event};

/// A welcomed input connection.
#[derive(Debug)]
pub struct Input {
    stream: UnixStream,
}

impl Input {
    /// Connects and says hello, panicking if the daemon will not talk.
    pub fn connect(socket: &Path) -> Input {
        let (stream, _) = connection::connect(socket, ConnectionKind::Input, "harness")
            .unwrap_or_else(|error| {
                panic!("could not open an input connection to {}: {error}", socket.display())
            });
        Input { stream }
    }

    pub fn send(&mut self, pane: &str, input: input_event::Input) {
        let event = proto::InputEvent { pane: pane.to_string(), input: Some(input) };
        connection::send(&mut self.stream, &event)
            .unwrap_or_else(|error| panic!("could not send input to the daemon: {error}"));
    }
}
