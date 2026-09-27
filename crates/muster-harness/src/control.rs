//! A control connection as a test drives one: ask, and see what came back first.
//!
//! Not the client Muster ships (`muster-daemon-client`, a later card). A test wants to see every
//! message in the order the daemon sent it, including the events that arrived before an answer,
//! which a real client folds away.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use muster_daemon_proto::connection::{self, HandshakeError};
use muster_daemon_proto::{self as proto, ConnectionKind, control_message, request::Service};

use crate::until::PATIENCE;

/// A welcomed control connection.
#[derive(Debug)]
pub struct Control {
    stream: UnixStream,
    welcome: proto::Welcome,
    next_id: u64,
}

/// An answer, and the events that arrived on the connection before it.
#[derive(Debug)]
pub struct Asked {
    pub answer: proto::Answer,
    pub events: Vec<proto::Event>,
}

impl Asked {
    /// The answer's outcome, as the enum rather than its number.
    pub fn outcome(&self) -> proto::Outcome {
        self.answer.outcome()
    }
}

impl Control {
    /// Connects and says hello, panicking if the daemon will not talk.
    pub fn connect(socket: &Path) -> Control {
        Control::try_connect(socket).unwrap_or_else(|error| {
            panic!("could not open a control connection to {}: {error}", socket.display())
        })
    }

    pub fn try_connect(socket: &Path) -> Result<Control, HandshakeError> {
        let (stream, welcome) = connection::connect(socket, ConnectionKind::Control, "harness")?;
        Ok(Control { stream, welcome, next_id: 0 })
    }

    pub fn welcome(&self) -> &proto::Welcome {
        &self.welcome
    }

    /// Sends a request without waiting, and returns its id.
    pub fn send(&mut self, service: Service) -> u64 {
        self.next_id += 1;
        let request = proto::Request { id: self.next_id, service: Some(service) };
        connection::send(&mut self.stream, &request)
            .unwrap_or_else(|error| panic!("could not send a request to the daemon: {error}"));
        self.next_id
    }

    /// Sends a request and reads until its answer, keeping the events that came first.
    pub fn ask(&mut self, service: Service) -> Asked {
        let id = self.send(service);
        let mut events = Vec::new();
        loop {
            match self.next_message(PATIENCE) {
                Some(control_message::Message::Event(event)) => events.push(event),
                Some(control_message::Message::Answer(answer)) if answer.id == id => {
                    return Asked { answer, events };
                }
                Some(control_message::Message::Answer(answer)) => {
                    panic!("an answer to request {} arrived while waiting for {id}", answer.id)
                }
                None => panic!(
                    "request {id} was not answered within {PATIENCE:?}; {} events arrived",
                    events.len()
                ),
            }
        }
    }

    /// The next event, which has to arrive within the suite's patience.
    pub fn next_event(&mut self) -> proto::Event {
        match self.next_message(PATIENCE) {
            Some(control_message::Message::Event(event)) => event,
            Some(control_message::Message::Answer(answer)) => {
                panic!("expected an event and got the answer to request {}", answer.id)
            }
            None => panic!("no event arrived within {PATIENCE:?}"),
        }
    }

    /// Whatever the daemon sends next, or `None` if nothing arrives within `within` or the
    /// daemon hung up.
    pub fn next_message(&mut self, within: Duration) -> Option<control_message::Message> {
        self.stream.set_read_timeout(Some(within)).expect("a socket takes a read timeout");
        match connection::receive::<proto::ControlMessage>(&mut self.stream) {
            Ok(Some(message)) => message.message,
            Ok(None) | Err(_) => None,
        }
    }
}
