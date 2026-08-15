//! muster-bridge: one pane's frame stream, unwrapped into one surface.
//!
//! libghostty gives a surface no way to be fed bytes, so the only channel into it is the
//! command it spawns (docs/observations/libghostty-9f9b8d1d.md section 2). This is that
//! command. Its stdout is the surface's PTY, and its job is to turn herdr's JSON frame
//! envelopes back into the ANSI they wrap.
//!
//! Output only, deliberately. The frames have already consumed the pane's terminal modes,
//! so nothing here may encode input - that belongs where the modes live, in the daemon. The
//! one thing this does write back is geometry.

mod pty;

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use muster_core::diagnostics::log::{self, LogLevel};
use muster_core::fields;
use muster_herdr::{ControlStreamMessage, FrameDecoder, PaneStreamEvent};

const USAGE: &str = "\
usage: muster-bridge <pane-id> [--control-socket <path>] [--herdr-socket <path>]
                     [--via-ssh <host> --ssh-control <path>]

Runs `herdr terminal session control <pane-id>` and unwraps its frames onto stdout.
Sized from the PTY on stdout, which is the surface's own geometry.

With --control-socket, dials that socket and relays whatever the app sends onto herdr's
control stream verbatim - input and scroll. Without it, the pane renders but cannot be
typed into.

With --herdr-socket, asks that daemon for the frames rather than whichever one this
process would find for itself. The app always says, because it runs a herdr of its own.

With --via-ssh, runs that command on another machine instead, over the ssh master the app
already opened for the daemon's control plane. Frames are byte-identical either way.";

/// herdr's stdin, which two threads write to: the resize watcher and the app's relay.
type HerdrInput = Arc<Mutex<ChildStdin>>;

fn main() {
    let Some(arguments) = Arguments::parse(&std::env::args().skip(1).collect::<Vec<_>>()) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };

    // The app names the file and every bridge it spawns inherits it, so one pane's whole
    // story - keystroke leaves the app, arrives here, goes to herdr - reads in order.
    log::start_from_environment(format!("bridge:{}", arguments.pane));

    // Before any thread exists, so they all inherit the blocked signal.
    let resizes = pty::watch_for_resize();
    let (columns, rows) = pty::terminal_size();

    log::info(
        "bridge.start",
        fields! {
            "pane" => arguments.pane,
            "cols" => columns,
            "rows" => rows,
            "control_socket" => arguments.control_socket.clone().unwrap_or("(none)".into()),
            "host" => arguments.ssh.as_ref()
                .map_or_else(|| "(this machine)".to_string(), |ssh| ssh.host.clone()),
        },
    );

    let mut herdr = match spawn_herdr(&arguments, columns, rows) {
        Ok(child) => child,
        Err(error) => {
            log::error(
                "bridge.herdr.failed",
                fields! { "pane" => arguments.pane, "error" => error },
            );
            eprint!(
                "muster-bridge: could not start herdr: {error}\n\
                 This pane will render nothing. Check that herdr is on PATH - on {} - and \
                 that the daemon there owns pane {}.\n\n",
                arguments.ssh.as_ref().map_or("this machine", |ssh| ssh.host.as_str()),
                arguments.pane
            );
            std::process::exit(1);
        }
    };

    let input: HerdrInput =
        Arc::new(Mutex::new(herdr.stdin.take().expect("herdr was spawned with a piped stdin")));
    let output = herdr.stdout.take().expect("herdr was spawned with a piped stdout");

    std::thread::spawn({
        let input = input.clone();
        move || {
            for () in resizes {
                let (columns, rows) = pty::terminal_size();
                send(&input, &ControlStreamMessage::Resize { columns, rows });
            }
        }
    });

    if let Some(path) = &arguments.control_socket {
        dial_the_app(path, &input);
    }

    pty::make_stdin_raw();
    pump_frames(output, &arguments.pane);
}

struct Arguments {
    pane: String,
    control_socket: Option<String>,
    /// Which daemon to ask for this pane's frames, when it is on this machine.
    ///
    /// Handed over rather than discovered, because Muster runs its own herdr on a session of
    /// its own: a bridge that found a daemon for itself would find whichever one the user
    /// last started, not hold this pane, and end its stream before a single frame.
    herdr_socket: Option<String>,
    ssh: Option<Ssh>,
}

/// The machine a pane lives on, when it is not this one.
#[derive(Clone)]
struct Ssh {
    host: String,
    /// The master the app opened for this daemon's control plane. Reusing it is what keeps a
    /// pane cheap: a window of fifteen remote panes pays for one handshake rather than
    /// fifteen.
    control_path: String,
}

impl Arguments {
    fn parse(arguments: &[String]) -> Option<Arguments> {
        let mut read = arguments.iter();
        let pane = read.next()?.clone();
        if pane.starts_with('-') {
            return None;
        }
        let mut parsed = Arguments { pane, control_socket: None, herdr_socket: None, ssh: None };
        let (mut host, mut control_path) = (None, None);
        while let Some(flag) = read.next() {
            let value = read.next()?.clone();
            match flag.as_str() {
                "--control-socket" => parsed.control_socket = Some(value),
                "--herdr-socket" => parsed.herdr_socket = Some(value),
                "--via-ssh" => host = Some(value),
                "--ssh-control" => control_path = Some(value),
                _ => return None,
            }
        }
        parsed.ssh = match (host, control_path) {
            (Some(host), Some(control_path)) => Some(Ssh { host, control_path }),
            // Half an ssh target is not a local pane, it is a mistake that would render the
            // wrong machine's terminal. Refusing prints the usage rather than quietly
            // attaching to whatever herdr this process can see.
            (None, None) => None,
            _ => return None,
        };
        Some(parsed)
    }
}

/// Starts the command whose frames become this pane.
///
/// Remotely it is the same command through ssh, which is why the frames are identical either
/// way: the daemon renders them, and the transport carries bytes. The master is reused rather
/// than reconnected, and batch mode is set for the same reason the app sets it - a pane that
/// stopped to ask for a password would hang with nothing to type into.
///
/// ssh joins everything after the destination and hands it to the far shell, so an argument
/// with a space in it would come apart. Nothing here has one: a pane id and two numbers.
fn spawn_herdr(arguments: &Arguments, columns: u16, rows: u16) -> Result<Child, String> {
    let mut command = match &arguments.ssh {
        None => {
            let mut command = Command::new(herdr_binary());
            // The daemon the app is talking to, rather than whichever one this process would
            // find. Muster runs its own under a session of its own, so a CLI left to look for
            // itself reaches a different daemon, does not hold this pane, and closes its
            // stream immediately - a pane that renders nothing and says nothing.
            if let Some(socket) = &arguments.herdr_socket {
                command.env("HERDR_SOCKET_PATH", socket);
            }
            command
        }
        Some(ssh) => {
            let mut command = Command::new("ssh");
            command.args(["-S", &ssh.control_path, "-o", "BatchMode=yes", &ssh.host, "herdr"]);
            command
        }
    };
    command
        .args(["terminal", "session", "control", &arguments.pane])
        .args(["--cols", &columns.to_string(), "--rows", &rows.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| error.to_string())
}

/// The herdr this Muster ships, which sits beside this binary.
///
/// The same rule the app uses to find this bridge, applied one step further along, and for
/// the same reason: a PATH lookup finds whatever version somebody installed, and the frames
/// on a pane's screen are then rendered by a daemon nobody pinned. Falls back to the name so
/// that a bridge run by hand still works, which is how this gets debugged.
fn herdr_binary() -> std::ffi::OsString {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|path| Some(path.parent()?.join("herdr")))
        .filter(|path| path.is_file());
    beside.map_or_else(|| "herdr".into(), Into::into)
}

fn send(input: &HerdrInput, message: &ControlStreamMessage) {
    let mut input = input.lock().expect("a panicking writer poisoned herdr's stdin");
    // Nowhere useful to report a failed write: herdr has gone, and the frame pump is about
    // to notice and say so with the reason it was given.
    let _ = input.write_all(&message.wire_format());
    let _ = input.flush();
}

/// Dials the app and relays whatever it sends, verbatim.
///
/// The app writes herdr's control-stream JSON, so this is a copy: keeping the bridge free
/// of any vocabulary of its own is what lets the adapter stay in one place
/// (architecture.md, the backend seam).
fn dial_the_app(path: &str, input: &HerdrInput) {
    let Ok(socket) = UnixStream::connect(path) else {
        log::error(
            "bridge.control.failed",
            fields! { "path" => path, "impact" => "this pane renders but swallows every keystroke" },
        );
        eprint!(
            "muster-bridge: could not reach the app on {path}\n\
             This pane will render but swallow every keystroke, which otherwise looks like a \
             dead terminal rather than a broken channel. Usual cause: the app closed the \
             socket, or this bridge outlived the window that spawned it.\n\n"
        );
        return;
    };

    log::info("bridge.control.dialed", fields! { "path" => path });
    let input = input.clone();
    std::thread::spawn(move || relay(socket, &input));
}

/// Lines are reassembled here rather than passed on as they arrive, because herdr parses
/// its stdin as newline-delimited JSON and half a message is a parse error that would
/// desynchronize everything after it.
fn relay(socket: UnixStream, input: &HerdrInput) {
    let mut reader = BufReader::new(socket);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            // The app is gone. The pane keeps rendering: sessions outlive the client.
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        log::debug(
            "bridge.relay",
            fields! {
                "bytes" => line.len(),
                "line" => if log::includes_input() {
                    String::from_utf8_lossy(&line).to_string()
                } else {
                    String::new()
                },
            },
        );
        let mut herdr = input.lock().expect("a panicking writer poisoned herdr's stdin");
        if herdr.write_all(&line).is_err() {
            return;
        }
        let _ = herdr.flush();
    }
}

/// Pumps decoded frames to the surface, until the stream ends.
fn pump_frames(mut output: impl Read, pane: &str) -> ! {
    let mut decoder = FrameDecoder::new();
    let mut pump = Pump::default();
    // Heap rather than stack: a repaint is routinely tens of kilobytes, and this thread
    // has no reason to carry that in its frame.
    let mut chunk = vec![0u8; 64 * 1024].into_boxed_slice();

    loop {
        let read = match output.read(&mut chunk) {
            Ok(0) | Err(_) => {
                // herdr hung up without a closing frame, which the protocol does not call
                // for. Same exit either way - it is what tells libghostty this pane's
                // command is gone - but it goes through the same reporting so the window
                // never just stops.
                pump.finish(pane, Some("herdr's stream ended without a closing frame"));
            }
            Ok(read) => read,
        };

        for event in decoder.consume(&chunk[..read]) {
            match event {
                PaneStreamEvent::Frame(frame) => pump.render(&frame.bytes),
                PaneStreamEvent::Closed { reason } => pump.finish(pane, reason.as_deref()),
            }
        }
    }
}

#[derive(Default)]
struct Pump {
    /// Whether anything was ever painted, which is what separates a pane that ended from a
    /// pane that never began.
    rendered: bool,
    /// Repaints since the last summary, and when that was.
    ///
    /// Frames are the answer to "did the pane react to what I typed", which is the first
    /// question anyone asks of this log and the one it could not answer: per-frame records
    /// sit at trace, off by default, because at repaint rates they bury everything else. A
    /// periodic count is legible at any rate and still lands within a second of the
    /// keystroke that caused it.
    frames_since_summary: u64,
    bytes_since_summary: usize,
    last_summary: u64,
}

const SUMMARY_INTERVAL_NS: u64 = 1_000_000_000;

impl Pump {
    fn render(&mut self, bytes: &[u8]) {
        // An attach opens with a full repaint, so a surface never has to have seen the
        // start of the stream.
        if !self.rendered {
            log::info("bridge.frame.first", fields! { "bytes" => bytes.len() });
            self.last_summary = muster_core::diagnostics::monotonic_now();
        }
        self.rendered = true;
        if log::enabled(LogLevel::Trace) {
            log::trace("bridge.frame", fields! { "bytes" => bytes.len() });
        }
        self.frames_since_summary += 1;
        self.bytes_since_summary += bytes.len();
        self.summarize_if_due();

        let mut out = std::io::stdout().lock();
        let _ = out.write_all(bytes);
        let _ = out.flush();
    }

    /// Emits a repaint count at most once a second, and only when there was one.
    ///
    /// Silence is information here: a second with no summary is a second the pane did not
    /// change, which is exactly what "I pressed a key and nothing happened" looks like from
    /// the outside.
    fn summarize_if_due(&mut self) {
        if muster_core::diagnostics::monotonic_since(self.last_summary) < SUMMARY_INTERVAL_NS {
            return;
        }
        log::debug(
            "bridge.frames",
            fields! { "frames" => self.frames_since_summary, "bytes" => self.bytes_since_summary },
        );
        self.frames_since_summary = 0;
        self.bytes_since_summary = 0;
        self.last_summary = muster_core::diagnostics::monotonic_now();
    }

    /// Reports why the stream ended, and exits.
    ///
    /// herdr states its reason in the closing frame and this process is the only thing that
    /// ever sees it. Exiting silently made a mistyped pane id and a pane the user closed
    /// into the same event: an empty window and ghostty's own "failed to launch" box, which
    /// blames the command rather than naming the pane that does not exist.
    fn finish(&self, pane: &str, reason: Option<&str>) -> ! {
        let why = reason.unwrap_or("herdr gave no reason");
        log::info(
            "bridge.closed",
            fields! { "pane" => pane, "reason" => why, "rendered" => self.rendered },
        );

        if self.rendered {
            eprintln!("muster-bridge: pane {pane} closed: {why}");
            std::process::exit(0);
        }

        log::error(
            "bridge.attach.failed",
            fields! {
                "pane" => pane,
                "reason" => why,
                "impact" => "this window stays empty for as long as it is open",
            },
        );
        eprint!(
            "muster-bridge: could not attach to pane {pane}: {why}\n\
             This window will stay empty. Most often the pane id is wrong or its workspace \
             is gone; `herdr pane list` names the panes that exist right now.\n\n"
        );
        // Nothing ever painted, so the attach itself failed. Non-zero because this is not a
        // session ending - it is a session that never started.
        std::process::exit(1);
    }
}
