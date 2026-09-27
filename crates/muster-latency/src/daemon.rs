//! The daemon being measured, spoken to the way the app will: panes made on the control
//! connection, keys sent on the input connection.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use muster_daemon_proto::connection;
use muster_daemon_proto::request::Service;
use muster_daemon_proto::{
    self as proto, ConnectionKind, answer, input_event, pane_request, placement, stream_message,
    stream_request,
};
use muster_harness::{Control, Input};
use prost::Message;

use crate::glyph;

/// The grid every measured pane has, the one `tools/latency.py` uses.
pub(crate) const GRID: proto::Grid =
    proto::Grid { cols: 80, rows: 24, width_px: 800, height_px: 480 };

pub(crate) struct Daemon {
    /// Held so the daemon lives as long as the run, when this run started it.
    _spawned: Option<muster_harness::Daemon>,
    socket: PathBuf,
    control: Control,
    input: Input,
    /// Unique to this run, so a daemon measured over `--socket` can already hold panes.
    prefix: String,
    made: u32,
}

impl Daemon {
    pub(crate) fn spawn(binary: &Path) -> Daemon {
        let spawned = muster_harness::Daemon::start(binary);
        let socket = spawned.socket_path().to_path_buf();
        Daemon::with(Some(spawned), socket)
    }

    pub(crate) fn at(socket: &Path) -> Daemon {
        Daemon::with(None, socket.to_path_buf())
    }

    fn with(spawned: Option<muster_harness::Daemon>, socket: PathBuf) -> Daemon {
        let control = Control::connect(&socket);
        let input = Input::connect(&socket);
        Daemon {
            _spawned: spawned,
            socket,
            control,
            input,
            prefix: format!("l{}", std::process::id()),
            made: 0,
        }
    }

    pub(crate) fn socket(&self) -> &Path {
        &self.socket
    }

    /// A pane of its own tab running `command`, and its name.
    pub(crate) fn pane(&mut self, command: &str) -> String {
        self.made += 1;
        let (pane, tab) =
            (format!("{}p{}", self.prefix, self.made), format!("{}t{}", self.prefix, self.made));
        let create = pane_request::Create {
            pane: pane.clone(),
            placement: Some(proto::Placement {
                r#where: Some(placement::Where::NewTab(placement::NewTab { tab, label: None })),
            }),
            grid: Some(GRID),
            command: Some(command.to_string()),
            ..pane_request::Create::default()
        };
        let asked = self.control.ask(Service::Pane(proto::PaneRequest {
            request: Some(pane_request::Request::Create(create)),
        }));
        assert_eq!(asked.outcome(), proto::Outcome::Done, "making a pane: {}", asked.answer.reason);
        pane
    }

    pub(crate) fn close(&mut self, pane: &str) {
        self.control.ask(Service::Pane(proto::PaneRequest {
            request: Some(pane_request::Request::Close(pane_request::Close {
                pane: pane.to_string(),
            })),
        }));
    }

    /// Types `letter` into `pane`, as a key press libghostty would carry.
    pub(crate) fn key(&mut self, pane: &str, letter: u8) {
        self.input.send(
            pane,
            input_event::Input::Key(input_event::Key {
                action: proto::KeyAction::Press.into(),
                // libghostty's key codes run a to z from 20.
                key: 20 + u32::from(letter - b'a'),
                text: char::from(letter).to_string(),
                unshifted_codepoint: u32::from(letter),
                ..input_event::Key::default()
            }),
        );
    }

    /// Types `line` into `pane` and presses Return, as `muster pane send` does.
    pub(crate) fn send_line(&mut self, pane: &str, line: &str) {
        self.input.send(
            pane,
            input_event::Input::Send(input_event::Send { text: line.to_string(), enter: true }),
        );
    }

    /// The rows of `pane`'s screen, as the daemon's own terminal has them.
    pub(crate) fn screen(&mut self, pane: &str) -> Vec<String> {
        let read = |control: &mut Control, first_row, rows| {
            let asked = control.ask(Service::Pane(proto::PaneRequest {
                request: Some(pane_request::Request::Read(pane_request::Read {
                    pane: pane.to_string(),
                    first_row,
                    rows,
                })),
            }));
            match asked.answer.detail {
                Some(answer::Detail::Text(text)) => text,
                other => panic!("reading {pane} answered {other:?}: {}", asked.answer.reason),
            }
        };
        let total = read(&mut self.control, 0, 1).total_rows;
        let first = total.saturating_sub(u64::from(GRID.rows));
        read(&mut self.control, first, GRID.rows).text.lines().map(str::to_string).collect()
    }
}

/// A pane's stream read directly, with no bridge: what the daemon's share of an echo costs.
pub(crate) struct Stream {
    stream: UnixStream,
    /// Bytes of output frames received since the last [`Stream::take_bytes`], framing included.
    framed: u64,
}

impl Stream {
    pub(crate) fn attach(socket: &Path, pane: &str) -> Stream {
        let (mut stream, _) = connection::connect(socket, ConnectionKind::Stream, "muster-latency")
            .unwrap_or_else(|error| {
                panic!("could not open a stream to {}: {error}", socket.display())
            });
        let attach =
            stream_request::Attach { pane: pane.to_string(), grid: Some(GRID), takeover: false };
        let request =
            proto::StreamRequest { request: Some(stream_request::Request::Attach(attach)) };
        connection::send(&mut stream, &request).expect("attaching");
        let mut attached = Stream { stream, framed: 0 };
        attached.settle();
        attached
    }

    /// Reads whatever has arrived, crediting it, until nothing has for a moment.
    pub(crate) fn settle(&mut self) {
        while self.next(Duration::from_millis(50)).is_some() {}
        self.framed = 0;
    }

    /// How long until `letter` shows in the pane's output, timed from `from`.
    pub(crate) fn wait_for(&mut self, letter: u8, from: Instant, timeout: Duration) -> Option<f64> {
        let deadline = from + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            if let Some(bytes) = self.next(left)?
                && glyph::shows(&bytes, letter)
            {
                return Some(from.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }

    pub(crate) fn take_bytes(&mut self) -> u64 {
        std::mem::take(&mut self.framed)
    }

    /// The next message's output, crediting it; `Some(None)` for anything else.
    #[allow(clippy::option_option)]
    fn next(&mut self, within: Duration) -> Option<Option<Vec<u8>>> {
        let _ = self.stream.set_read_timeout(Some(within.max(Duration::from_micros(100))));
        let message = connection::receive::<proto::StreamMessage>(&mut self.stream).ok()??;
        let framed = 4 + message.encoded_len() as u64;
        match message.message {
            Some(stream_message::Message::Output(bytes)) => {
                self.framed += framed;
                let credit = stream_request::Credit { bytes: bytes.len() as u64 };
                let request =
                    proto::StreamRequest { request: Some(stream_request::Request::Credit(credit)) };
                let _ = connection::send(&mut self.stream, &request);
                Some(Some(bytes))
            }
            _ => Some(None),
        }
    }
}
