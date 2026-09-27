//! `muster-daemon replace`: hands a running daemon's panes to another daemon, by hand. The app
//! asks for the same over its control connection when it finds an older daemon running.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use muster_daemon_proto::{self as proto, ConnectionKind, connection, install, session_request};

/// How long the running daemon may take: each step of a handoff waits at most ten seconds, and
/// holding a pane's reader is one of them.
const PATIENCE: Duration = Duration::from_mins(2);

const USAGE: &str = "usage: muster-daemon replace [--socket PATH] [--program PATH] [--data DIR]\n\n\
    Hands every pane of the daemon on the socket to a new daemon, which serves the same socket \
    from then on, and ends none of them. The socket is this install's daemon's unless --socket \
    names another. The new daemon is --program, or the executable the running daemon started \
    from; --data is its data directory, or the one it finds beside itself. Every client is \
    disconnected and connects again. If anything fails, the running daemon goes on as it was.";

pub(crate) fn run(arguments: impl Iterator<Item = String>) -> ExitCode {
    let mut arguments = arguments;
    let mut socket = None;
    let mut replace = session_request::Replace::default();
    while let Some(argument) = arguments.next() {
        let value = match argument.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--socket" | "--program" | "--data" => match arguments.next() {
                Some(value) => value,
                None => return usage(&format!("{argument} needs a path")),
            },
            other => return usage(&format!("{other} is not an option")),
        };
        match argument.as_str() {
            "--socket" => socket = Some(PathBuf::from(value)),
            "--program" => replace.program = Some(value),
            _ => replace.data = Some(value),
        }
    }
    let Some(socket) = socket.or_else(|| {
        install::muster_home(|name| std::env::var(name).ok())
            .map(|home| install::socket_path(&home))
    }) else {
        return usage("neither MUSTER_HOME nor HOME is set, so there is no default socket");
    };
    match ask(&socket, replace) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("muster-daemon replace: {problem}");
            ExitCode::FAILURE
        }
    }
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("muster-daemon replace: {problem}\n{USAGE}");
    ExitCode::from(2)
}

fn ask(socket: &std::path::Path, replace: session_request::Replace) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|error| format!("no daemon at {}: {error}", socket.display()))?;
    connection::open(&mut stream, ConnectionKind::Control, "muster-daemon replace")
        .map_err(|error| error.to_string())?;
    stream.set_read_timeout(Some(PATIENCE)).map_err(|error| error.to_string())?;
    let request = proto::Request {
        id: 1,
        service: Some(proto::request::Service::Session(proto::SessionRequest {
            request: Some(session_request::Request::Replace(replace)),
        })),
    };
    connection::send(&mut stream, &request).map_err(|error| error.to_string())?;
    // Not subscribed, so the one message coming is the answer.
    let answer = match connection::receive::<proto::ControlMessage>(&mut stream) {
        Ok(Some(proto::ControlMessage {
            message: Some(proto::control_message::Message::Answer(answer)),
        })) => answer,
        Ok(_) => return Err("the daemon hung up without answering".to_string()),
        Err(error) => return Err(format!("the daemon did not answer: {error}")),
    };
    match answer.outcome() {
        proto::Outcome::Done => Ok(()),
        _ => Err(answer.reason),
    }
}
