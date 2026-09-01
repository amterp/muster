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
use muster_core::diagnostics::poison;
use muster_core::fields;
use muster_herdr::{ControlStreamMessage, FrameDecoder, PaneStreamEvent};
use muster_ssh::quoted;

const USAGE: &str = "\
usage: muster-bridge <pane-id> [--control-socket <path>] [--herdr-socket <path>]
                     [--herdr-binary <path>] [--via-ssh <host> --ssh-control <path>]

Runs `herdr terminal session control <pane-id>` and unwraps its frames onto stdout.
Sized from the PTY on stdout, which is the surface's own geometry.

With --control-socket, dials that socket and relays whatever the app sends onto herdr's
control stream verbatim - input and scroll. Without it, the pane renders but cannot be
typed into.

With --herdr-socket, asks that daemon for the frames rather than whichever one this
process would find for itself. The app always says, because it runs a herdr of its own.
With --via-ssh the path is the far machine's, since that is where the CLI runs.

With --herdr-binary, runs that daemon rather than looking for one. The app always says,
because only the app knows where this build put it. Without it this looks beside its own
executable and then on PATH, which is right for a bridge run by hand and right for a dev
build, and cannot work inside a shipped bundle.

With --via-ssh, runs that command on another machine instead, over the ssh master the app
already opened for the daemon's control plane. Frames are byte-identical either way. It
prefers the herdr Muster installed there, at ~/.muster/bin/herdr, and falls back to
whatever is on PATH.";

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
            // Which herdr renders this pane, which is the first question when a pane paints
            // nothing and the one that cost a release to answer (kan a_2Hnh3g0Y5). Absent for
            // a remote pane: that one is resolved by the script the far shell runs.
            "herdr" => match &arguments.ssh {
                Some(_) => "(the far machine's)".to_string(),
                None => herdr_binary(arguments.herdr_binary.as_deref())
                    .to_string_lossy()
                    .into_owned(),
            },
        },
    );

    let mut herdr = match spawn_herdr(&arguments, columns, rows) {
        Ok(child) => child,
        Err(error) => {
            // The path is on the record because without it this failure names no file, and
            // then a released bundle that renders nothing in every pane says only "No such
            // file or directory" - which is what kan a_2Hnh3g0Y5 cost to work out by hand.
            let tried = match &arguments.ssh {
                Some(_) => "(the far machine's)".to_string(),
                None => {
                    herdr_binary(arguments.herdr_binary.as_deref()).to_string_lossy().into_owned()
                }
            };
            log::error(
                "bridge.herdr.failed",
                fields! { "pane" => arguments.pane, "herdr" => tried.clone(), "error" => error },
            );
            eprint!(
                "muster-bridge: could not start herdr at {tried}: {error}\n\
                 This pane will render nothing. Check that a herdr can be run on {} - \
                 Muster installs its own under ~/.muster - and that the daemon there owns \
                 pane {}.\n\n",
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
                // Logged because a resize that goes nowhere is invisible otherwise: the pane
                // keeps rendering at its old geometry and a full-screen program redraws into
                // a grid the wrong shape, which reads as a broken TUI rather than as a
                // message that never arrived.
                log::info("bridge.resize", fields! { "cols" => columns, "rows" => rows });
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
    /// Which daemon to ask for this pane's frames.
    ///
    /// Handed over rather than discovered, because Muster runs its own herdr on a session of
    /// its own: a bridge that found a daemon for itself would find whichever one the user
    /// last started, not hold this pane, and end its stream before a single frame.
    ///
    /// Spelled the way the machine that will open it spells it, which is the far machine's
    /// path when this bridge is running its CLI over ssh.
    herdr_socket: Option<String>,
    /// Which daemon binary to run, on this machine.
    ///
    /// Handed over for the same reason the socket is, one question further along: where a
    /// build put its herdr is a packaging question, and the app is the only process that
    /// knows the answer. A bridge that worked it out for itself was right for a dev build and
    /// wrong for every shipped bundle, which is kan a_2Hnh3g0Y5 - the daemon moved into a
    /// helper bundle, nothing beside the bridge, and no PATH a Launch Services app could use.
    ///
    /// `None` for a bridge somebody ran by hand, which is how this gets debugged, and for a
    /// remote pane - that one runs its CLI over there and resolves it over there.
    herdr_binary: Option<String>,
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
        let mut parsed = Arguments {
            pane,
            control_socket: None,
            herdr_socket: None,
            herdr_binary: None,
            ssh: None,
        };
        let (mut host, mut control_path) = (None, None);
        while let Some(flag) = read.next() {
            let value = read.next()?.clone();
            match flag.as_str() {
                "--control-socket" => parsed.control_socket = Some(value),
                "--herdr-socket" => parsed.herdr_socket = Some(value),
                "--herdr-binary" => parsed.herdr_binary = Some(value),
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
/// ssh joins everything after the destination and hands it to the far shell, so the remote form
/// is one shell script quoted whole rather than an argument vector - and the values inside it
/// are quoted again, because that script is parsed a second time when it runs.
fn spawn_herdr(arguments: &Arguments, columns: u16, rows: u16) -> Result<Child, String> {
    let mut command = match &arguments.ssh {
        None => {
            let mut command = Command::new(herdr_binary(arguments.herdr_binary.as_deref()));
            // The daemon the app is talking to, rather than whichever one this process would
            // find. Muster runs its own under a session of its own, so a CLI left to look for
            // itself reaches a different daemon, does not hold this pane, and closes its
            // stream immediately - a pane that renders nothing and says nothing.
            if let Some(socket) = &arguments.herdr_socket {
                command.env("HERDR_SOCKET_PATH", socket);
            }
            command.args(["terminal", "session", "control", &arguments.pane]).args([
                "--cols",
                &columns.to_string(),
                "--rows",
                &rows.to_string(),
            ]);
            command
        }
        Some(ssh) => {
            let mut command = Command::new("ssh");
            command.args([
                "-S",
                &ssh.control_path,
                "-o",
                "BatchMode=yes",
                &ssh.host,
                "sh",
                "-c",
                &quoted(&far_side_command(arguments, columns, rows)),
            ]);
            command
        }
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| error.to_string())
}

/// The script that produces a remote pane's frames, run by the far machine's shell.
///
/// Two things it has to get right, and both are the same problem the local arm solves by being
/// handed values it could not work out.
///
/// **Which herdr.** Muster puts its own on a machine it attaches to, at a path under that
/// machine's home, and it is deliberately not on anybody's PATH - so a bare `herdr` finds
/// whatever is installed there, and on a devenv set up the way Muster intends, finds nothing at
/// all. This prefers Muster's own and falls back to the name, which is what a machine holding
/// somebody else's daemon has. Preferring the pinned one is the same argument the local bridge
/// makes for running the herdr beside itself: the frames on a pane's screen should be rendered
/// by a daemon somebody pinned.
///
/// **Which daemon.** Muster's remote daemon listens on a herdr session of its own, exactly as
/// the local one does, so the CLI has to be told rather than left to look - and told the path as
/// the *far* side spells it, which is why the app now sends that rather than the near end of the
/// tunnel.
fn far_side_command(arguments: &Arguments, columns: u16, rows: u16) -> String {
    let daemon = arguments.herdr_socket.as_ref().map_or_else(String::new, |socket| {
        format!("export HERDR_SOCKET_PATH={}; ", quoted(socket))
    });
    format!(
        "H=\"$HOME/.muster/bin/herdr\"; [ -x \"$H\" ] || H=herdr; {daemon}exec \"$H\" \
         terminal session control {} --cols {columns} --rows {rows}",
        quoted(&arguments.pane),
    )
}

/// The herdr this pane's frames should come from.
///
/// The app's answer where there is one, because only the app knows where this build put its
/// daemon: a dev build stages it beside the binaries, and a shipped bundle carries it as a
/// helper application in `Contents/Library/`. A bridge that decided for itself was right about
/// the first and wrong about the second, and the released cask rendered nothing in every pane
/// (kan a_2Hnh3g0Y5).
///
/// The two fallbacks are for a bridge somebody ran by hand, which is how this gets debugged.
/// Beside this binary first, because that is where `./dev` stages the pinned daemon and a PATH
/// lookup would find whatever version happens to be installed - the frames on a pane's screen
/// should be rendered by a daemon somebody pinned. Nothing here rescues a bundle: an app
/// opened by Launch Services is handed launchd's `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, and
/// every directory on it is SIP-protected.
fn herdr_binary(told: Option<&str>) -> std::ffi::OsString {
    if let Some(path) = told.filter(|path| !path.is_empty()) {
        return path.into();
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|path| Some(path.parent()?.join("herdr")))
        .filter(|path| path.is_file());
    beside.map_or_else(|| "herdr".into(), Into::into)
}

fn send(input: &HerdrInput, message: &ControlStreamMessage) {
    let mut input = poison::lock(input, "herdr-stdin");
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
        let mut herdr = poison::lock(input, "herdr-stdin");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn remotely(pane: &str, socket: Option<&str>) -> String {
        let arguments = Arguments {
            pane: pane.to_string(),
            control_socket: None,
            herdr_socket: socket.map(ToString::to_string),
            // Never sent for a remote pane: a path on this machine names nothing over there.
            herdr_binary: None,
            ssh: Some(Ssh { host: "devenv".to_string(), control_path: "/tmp/c".to_string() }),
        };
        far_side_command(&arguments, 80, 24)
    }

    #[test]
    fn a_remote_pane_prefers_the_herdr_muster_put_there() {
        let script = remotely("p1w3r07bsd", Some("/home/dev/.config/herdr/s/herdr.sock"));
        assert!(
            script.starts_with(r#"H="$HOME/.muster/bin/herdr"; [ -x "$H" ] || H=herdr; "#),
            "the script should try Muster's own first and fall back to the name: {script}"
        );
    }

    #[test]
    fn a_remote_pane_is_told_which_daemon_rather_than_looking() {
        // The one that stops a remote pane rendering nothing: Muster's daemon listens on a
        // session of its own over there too, so a CLI left to find one reaches nothing.
        let script = remotely("p1w3r07bsd", Some("/home/dev/.config/herdr/s/herdr.sock"));
        assert!(
            script.contains("export HERDR_SOCKET_PATH='/home/dev/.config/herdr/s/herdr.sock'; "),
            "the far side should be told the path as it spells it: {script}"
        );
        assert!(
            script.ends_with(
                r#"exec "$H" terminal session control 'p1w3r07bsd' --cols 80 --rows 24"#
            ),
            "and then run the pane's stream: {script}"
        );
    }

    #[test]
    fn a_daemon_nobody_named_a_socket_for_leaves_the_cli_to_find_it() {
        let script = remotely("p1w3r07bsd", None);
        assert!(!script.contains("HERDR_SOCKET_PATH"), "nothing to say: {script}");
    }

    #[test]
    fn a_real_shell_reads_the_quoted_script_back_as_one_word() {
        // The claim the remote arm rests on, checked against a shell rather than against a
        // belief about escaping: ssh hands the far machine a command line, so the script has
        // to survive one parse whole - including a home directory with a quote in it, which
        // is the case hand-reasoning gets wrong.
        let script = remotely("p1w3r07bsd", Some("/home/dev/it's/herdr.sock"));
        let read = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf %s {}", quoted(&script)))
            .output()
            .expect("/bin/sh should run");
        assert_eq!(
            String::from_utf8_lossy(&read.stdout),
            script,
            "a shell should read back exactly what was quoted"
        );
    }
}
