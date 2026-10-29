//! An API connection as a test drives one (MIP-10): the client every test of the daemon's API is
//! written with.
//!
//! Like [`crate::Control`], it shows a test every message in the order the daemon sent it,
//! including the events that arrived before an answer, and it runs in the test's process, apart
//! from the daemon's, as any program using the API does.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use muster_api as api;
use muster_api::connection::{self, HandshakeError};
use prost::Message;

use crate::until::PATIENCE;

/// A welcomed API connection.
#[derive(Debug)]
pub struct Api {
    stream: UnixStream,
    welcome: api::Welcome,
    next: u64,
}

/// An answer, and the events that arrived on the connection before it.
#[derive(Debug)]
pub struct ApiAsked {
    pub answer: api::Answer,
    pub events: Vec<api::Event>,
}

impl ApiAsked {
    pub fn outcome(&self) -> api::Outcome {
        self.answer.outcome()
    }

    pub fn refusal(&self) -> api::Refusal {
        self.answer.refusal()
    }

    /// The snapshot the answer carries, which a subscription's always does.
    pub fn snapshot(&self) -> &api::Snapshot {
        match &self.answer.detail {
            Some(api::answer::Detail::Snapshot(snapshot)) => snapshot,
            _ => panic!("the answer carries no snapshot: {:?}", self.answer),
        }
    }

    /// The page of history the answer carries.
    pub fn page(&self) -> &api::Page {
        match &self.answer.detail {
            Some(api::answer::Detail::Page(page)) => page,
            _ => panic!("the answer carries no page: {:?}", self.answer),
        }
    }
}

/// A page of `group`'s log: the newest entries after `after` and through `through`.
pub fn history(group: &str, after: u64, through: u64) -> api::request::Request {
    api::request::Request::Msg(api::MsgRequest {
        request: Some(api::msg_request::Request::History(api::History {
            group: group.to_string(),
            after,
            through,
        })),
    })
}

/// What came of a request whose answer may never arrive, as a client concludes it.
#[derive(Debug)]
#[allow(clippy::large_enum_variant, reason = "a test's result, matched at once and never held")]
pub enum Tried {
    Answered(ApiAsked),
    /// The connection ended after the request was written and before its answer: the request
    /// may have happened. The ending, when the daemon sent one.
    Unknown(Option<api::Ended>),
}

/// What reading the connection came to.
#[expect(clippy::large_enum_variant, reason = "one is read at a time and matched at once")]
enum Read {
    Message(api::daemon_message::Message),
    TimedOut,
    HungUp,
}

/// The client every test names itself as.
pub fn test_client() -> api::Client {
    api::Client { name: "harness".to_string(), version: env!("CARGO_PKG_VERSION").to_string() }
}

/// A subscription to `topics`.
pub fn subscribe_to(topics: &[api::Topic]) -> api::request::Request {
    api::request::Request::Api(api::ApiRequest {
        request: Some(api::api_request::Request::Subscribe(api::Subscribe {
            topics: topics.iter().map(|topic| (*topic).into()).collect(),
        })),
    })
}

/// A `pane.new` asking for `asked`.
pub fn new_pane(asked: api::NewPane) -> api::request::Request {
    api::request::Request::Pane(api::PaneRequest {
        request: Some(api::pane_request::Request::New(asked)),
    })
}

impl Api {
    /// Connects and says hello, panicking if the daemon will not serve this client.
    pub fn connect(socket: &Path) -> Api {
        Api::try_connect(socket).unwrap_or_else(|error| {
            panic!("could not open an API connection to {}: {error}", socket.display())
        })
    }

    pub fn try_connect(socket: &Path) -> Result<Api, HandshakeError> {
        Api::try_connect_as(socket, test_client())
    }

    /// Connects as `client`, whose name scopes the operation ids it sends.
    pub fn connect_as(socket: &Path, client: api::Client) -> Api {
        Api::try_connect_as(socket, client).unwrap_or_else(|error| {
            panic!("could not open an API connection to {}: {error}", socket.display())
        })
    }

    fn try_connect_as(socket: &Path, client: api::Client) -> Result<Api, HandshakeError> {
        let (stream, welcome) = connection::connect(socket, client)?;
        Ok(Api { stream, welcome, next: 0 })
    }

    /// Connects and sends `hello` as the first frame as it is, for a test of the handshake
    /// itself.
    pub fn try_connect_with_hello(socket: &Path, hello: &[u8]) -> Result<Api, HandshakeError> {
        let mut stream = UnixStream::connect(socket)
            .map_err(|error| HandshakeError::Unreachable(error.to_string()))?;
        muster_frame::write_frame(&mut stream, hello)
            .map_err(|error| HandshakeError::Unreachable(error.to_string()))?;
        let welcome = connection::answered(&mut stream)?;
        Ok(Api { stream, welcome, next: 0 })
    }

    pub fn welcome(&self) -> &api::Welcome {
        &self.welcome
    }

    /// This connection's socket, cloned, for a test that sends from another thread.
    pub fn writer(&self) -> UnixStream {
        self.stream.try_clone().expect("the connection's socket clones")
    }

    /// Sends a request without waiting, and returns the correlation id it carries.
    pub fn send(&mut self, request: api::request::Request) -> String {
        self.next += 1;
        let correlation = format!("harness-{}", self.next);
        let request = api::Request { correlation: correlation.clone(), request: Some(request) };
        connection::send(&mut self.stream, &request)
            .unwrap_or_else(|error| panic!("could not send a request to the daemon: {error}"));
        correlation
    }

    /// Sends a request and reads until its answer, keeping the events that came first.
    pub fn ask(&mut self, request: api::request::Request) -> ApiAsked {
        let correlation = self.send(request);
        let mut events = Vec::new();
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.read(deadline) {
                Read::Message(api::daemon_message::Message::Event(event)) => events.push(event),
                Read::Message(api::daemon_message::Message::Answer(answer))
                    if answer.correlation == correlation =>
                {
                    return ApiAsked { answer, events };
                }
                Read::Message(other) => {
                    panic!("waiting for the answer to {correlation}, the daemon sent {other:?}")
                }
                Read::TimedOut => panic!(
                    "{correlation} was not answered within {PATIENCE:?}; {} events arrived",
                    events.len()
                ),
                Read::HungUp => panic!(
                    "the daemon hung up before answering {correlation}; {} events arrived",
                    events.len()
                ),
            }
        }
    }

    /// Sends a request and reads until its answer or the connection's end, which leaves the
    /// request's fate unknown. Events that arrive first are kept with the answer.
    pub fn try_ask(&mut self, request: api::request::Request) -> Tried {
        let correlation = self.send(request);
        let mut events = Vec::new();
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.read(deadline) {
                Read::Message(api::daemon_message::Message::Event(event)) => events.push(event),
                Read::Message(api::daemon_message::Message::Answer(answer))
                    if answer.correlation == correlation =>
                {
                    return Tried::Answered(ApiAsked { answer, events });
                }
                Read::Message(api::daemon_message::Message::Ended(ended)) => {
                    return Tried::Unknown(Some(ended));
                }
                Read::HungUp => return Tried::Unknown(None),
                Read::Message(other) => {
                    panic!("waiting for the answer to {correlation}, the daemon sent {other:?}")
                }
                Read::TimedOut => panic!("{correlation} was not answered within {PATIENCE:?}"),
            }
        }
    }

    /// Sends a request and hangs up without reading, so its answer is lost.
    pub fn send_and_hang_up(mut self, request: api::request::Request) {
        self.send(request);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    /// Subscribes to `topics`, which the daemon has to do.
    pub fn subscribe(&mut self, topics: &[api::Topic]) -> ApiAsked {
        let asked = self.ask(subscribe_to(topics));
        assert_eq!(asked.outcome(), api::Outcome::Done, "the subscription: {:?}", asked.answer);
        asked
    }

    /// The next event, which has to arrive within the suite's patience.
    pub fn next_event(&mut self) -> api::Event {
        self.event_matching("an event", |_| true)
    }

    /// The first event `matching` picks out, passing over the others, within the suite's
    /// patience.
    pub fn event_matching(
        &mut self,
        what: &str,
        mut matching: impl FnMut(&api::Event) -> bool,
    ) -> api::Event {
        let deadline = Instant::now() + PATIENCE;
        let mut passed = Vec::new();
        loop {
            match self.read(deadline) {
                Read::Message(api::daemon_message::Message::Event(event)) if matching(&event) => {
                    return event;
                }
                Read::Message(api::daemon_message::Message::Event(event)) => passed.push(event),
                Read::Message(other) => panic!("waiting for {what}, the daemon sent {other:?}"),
                Read::TimedOut => {
                    panic!("{what} did not arrive within {PATIENCE:?}; passed over {passed:?}")
                }
                Read::HungUp => {
                    panic!("the daemon hung up before {what}; passed over {passed:?}")
                }
            }
        }
    }

    /// The next answer, event or ending, or `None` if none arrives within `within` or the daemon
    /// hung up.
    pub fn next_message(&mut self, within: Duration) -> Option<api::daemon_message::Message> {
        match self.read(Instant::now() + within) {
            Read::Message(message) => Some(message),
            Read::TimedOut | Read::HungUp => None,
        }
    }

    /// Reads to the daemon's last word, returning it with the answers and events before it, and
    /// checks that the daemon then hangs up.
    pub fn ended(&mut self) -> (api::Ended, Vec<api::daemon_message::Message>) {
        let deadline = Instant::now() + PATIENCE;
        let mut before = Vec::new();
        loop {
            match self.read(deadline) {
                Read::Message(api::daemon_message::Message::Ended(ended)) => {
                    assert!(self.hangs_up(), "the daemon sent more after {ended:?}");
                    return (ended, before);
                }
                Read::Message(message) => before.push(message),
                Read::TimedOut => panic!(
                    "the connection did not end within {PATIENCE:?}; {} messages arrived",
                    before.len()
                ),
                Read::HungUp => {
                    panic!("the daemon hung up without saying why, after {} messages", before.len())
                }
            }
        }
    }

    /// Whether the daemon hangs up within the suite's patience, sending nothing first.
    pub fn hangs_up(&mut self) -> bool {
        matches!(self.read(Instant::now() + PATIENCE), Read::HungUp)
    }

    fn read(&mut self, deadline: Instant) -> Read {
        if !self.readable_before(deadline) {
            return Read::TimedOut;
        }
        // Something has arrived, and the daemon writes a frame whole, so the rest of it follows.
        let _ = self.stream.set_read_timeout(Some(PATIENCE));
        match muster_frame::read_frame_or_end(&mut self.stream, connection::LARGEST_MESSAGE) {
            Ok(Some(frame)) => match api::DaemonMessage::decode(frame.as_slice()) {
                Ok(api::DaemonMessage { message: Some(message) }) => Read::Message(message),
                Ok(api::DaemonMessage { message: None }) => {
                    panic!("the daemon sent a message with nothing in it")
                }
                Err(error) => panic!("a {}-byte frame did not decode: {error}", frame.len()),
            },
            Ok(None) | Err(_) => Read::HungUp,
        }
    }

    /// Whether the socket has something to read, the end included, before `deadline`.
    fn readable_before(&self, deadline: Instant) -> bool {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ms = i32::try_from(left.as_millis()).unwrap_or(i32::MAX);
            let mut watched =
                libc::pollfd { fd: self.stream.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: `watched` is one valid pollfd, for the length given.
            match unsafe { libc::poll(&raw mut watched, 1, ms) } {
                0 => return false,
                -1 if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {
                }
                _ => return true,
            }
        }
    }
}
