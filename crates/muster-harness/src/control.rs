//! A control connection as a test drives one: ask, and see what came back first.
//!
//! Not the client Muster ships (`muster-daemon-client`, a later card). A test wants to see every
//! message in the order the daemon sent it, including the events that arrived before an answer,
//! which a real client folds away.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use muster_daemon_proto::connection::{self, HandshakeError};
use muster_daemon_proto::{self as proto, ConnectionKind, control_message, request::Service};

use crate::until::PATIENCE;

/// A welcomed control connection.
#[derive(Debug)]
pub struct Control {
    stream: UnixStream,
    welcome: proto::Welcome,
    next_id: u64,
    /// The daemon's log records this connection follows, set apart as they arrive so they never
    /// stand between a test and the answer or event it waits for.
    logged: Vec<proto::LogLine>,
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
        Ok(Control { stream, welcome, next_id: 0, logged: Vec::new() })
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
                Some(control_message::Message::LogLine(_)) => {
                    unreachable!("next_message sets log lines apart")
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
            Some(control_message::Message::LogLine(_)) => {
                unreachable!("next_message sets log lines apart")
            }
            None => panic!("no event arrived within {PATIENCE:?}"),
        }
    }

    /// The next answer or event, or `None` if neither arrives within `within` or the daemon
    /// hung up. Log lines that arrive meanwhile are kept for [`Control::logged`].
    pub fn next_message(&mut self, within: Duration) -> Option<control_message::Message> {
        let deadline = Instant::now() + within;
        loop {
            match self.next_frame(deadline)? {
                Frame::Logged => {}
                Frame::Message(message) => return Some(*message),
            }
        }
    }

    /// The next frame to arrive before `deadline`, keeping a log line rather than returning it.
    fn next_frame(&mut self, deadline: Instant) -> Option<Frame> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        // macOS refuses a timeout on a socket whose peer has hung up, and reading one never
        // blocks: what was sent before it hung up still arrives, then the end.
        let _ = self.stream.set_read_timeout(Some(left));
        match connection::receive::<proto::ControlMessage>(&mut self.stream) {
            Ok(Some(proto::ControlMessage {
                message: Some(control_message::Message::LogLine(line)),
            })) => {
                self.logged.push(line);
                Some(Frame::Logged)
            }
            Ok(Some(proto::ControlMessage { message: Some(message) })) => {
                Some(Frame::Message(Box::new(message)))
            }
            Ok(Some(proto::ControlMessage { message: None }) | None) | Err(_) => None,
        }
    }

    /// The log lines that have arrived on this connection so far, reading for up to `within`
    /// more until one holds `needle`. Every line read stays here.
    pub fn logged_until(&mut self, needle: &str, within: Duration) -> &[proto::LogLine] {
        self.logged_times_until(needle, 1, within)
    }

    /// [`Control::logged_until`], until `times` lines hold `needle`.
    pub fn logged_times_until(
        &mut self,
        needle: &str,
        times: usize,
        within: Duration,
    ) -> &[proto::LogLine] {
        let deadline = Instant::now() + within;
        while self.logged.iter().filter(|line| line.line.contains(needle)).count() < times {
            match self.next_frame(deadline) {
                None => break,
                Some(Frame::Logged) => {}
                Some(Frame::Message(message)) => {
                    panic!("waiting for the log, the daemon sent {message:?}")
                }
            }
        }
        &self.logged
    }

    /// The log lines that have arrived so far, reading for up to `within` more until the line
    /// numbered `number` is among them. A daemon numbers its lines from 1 with no gaps, and its
    /// answer to `FollowLog` says the number of the last one it replays, so this is how a caller
    /// knows the replay has all arrived.
    pub fn logged_through(&mut self, number: u64, within: Duration) -> &[proto::LogLine] {
        let deadline = Instant::now() + within;
        while self.logged.last().is_none_or(|line| line.number < number) {
            match self.next_frame(deadline) {
                None => break,
                Some(Frame::Logged) => {}
                Some(Frame::Message(message)) => {
                    panic!("waiting for the log, the daemon sent {message:?}")
                }
            }
        }
        &self.logged
    }
}

enum Frame {
    Logged,
    Message(Box<control_message::Message>),
}
