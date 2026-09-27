//! Accepting connections, and the handshake that opens each one.

use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::time::Duration;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection;
use muster_daemon_proto::version::{PROTOCOL, compatible};
use muster_daemon_proto::{self as proto, ConnectionKind, hello_answer, install};

use crate::control;
use crate::session::Shared;
use crate::stream;

/// How long a connection may take to say hello. Past it, whatever dialed is not a Muster client
/// and is not worth a thread.
const HELLO_PATIENCE: Duration = Duration::from_secs(5);

/// Serves every connection to `listener`, each on a thread of its own, for as long as the daemon
/// runs.
pub(crate) fn accept(listener: &UnixListener, shared: &Arc<Shared>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = Arc::clone(shared);
        let spawned = std::thread::Builder::new()
            .name("connection".to_string())
            .spawn(move || open(stream, &shared));
        if let Err(error) = spawned {
            log::error(
                "daemon.connection.no_thread",
                fields! {
                    "error" => error,
                    "impact" => "a client's connection was closed unanswered",
                    "check" => "the daemon's thread count; a leak shows as one per pane or client",
                },
            );
        }
    }
}

fn open(mut stream: UnixStream, shared: &Arc<Shared>) {
    let _ = stream.set_read_timeout(Some(HELLO_PATIENCE));
    let hello = match connection::receive::<proto::Hello>(&mut stream) {
        Ok(Some(hello)) => hello,
        Ok(None) => return,
        Err(error) => {
            log::warn(
                "daemon.connection.no_hello",
                fields! {
                    "error" => error,
                    "impact" => "the connection was closed; whatever dialed was not answered",
                    "check" => "whether something other than Muster is dialing the daemon's socket",
                },
            );
            return;
        }
    };
    let answer = match judge(&hello) {
        Ok(()) => hello_answer::Answer::Welcome(proto::Welcome {
            protocol: Some(PROTOCOL),
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            install: install::INSTALL.to_string(),
            instance: shared.instance,
            pid: std::process::id(),
        }),
        Err(reason) => {
            log::warn(
                "daemon.connection.refused",
                fields! {
                    "client" => hello.client,
                    "reason" => reason,
                    "impact" => "that client cannot use this daemon",
                },
            );
            hello_answer::Answer::Refused(proto::HelloRefused { reason, daemon: Some(PROTOCOL) })
        }
    };
    let welcomed = matches!(answer, hello_answer::Answer::Welcome(_));
    let answer = proto::HelloAnswer { answer: Some(answer) };
    if connection::send(&mut stream, &answer).is_err() || !welcomed {
        return;
    }
    let _ = stream.set_read_timeout(None);
    match ConnectionKind::try_from(hello.kind) {
        Ok(ConnectionKind::Stream) => stream::serve(stream, shared),
        _ => control::serve(stream, shared, &hello.client),
    }
}

/// Whether this daemon will serve a connection that says hello this way.
fn judge(hello: &proto::Hello) -> Result<(), String> {
    let Some(theirs) = hello.protocol else {
        return Err("the hello names no protocol version".to_string());
    };
    if !compatible(&PROTOCOL, &theirs) {
        return Err(format!(
            "this daemon speaks protocol {PROTOCOL} and the client speaks {theirs}; a client \
             talks only to a daemon of its own major version"
        ));
    }
    match ConnectionKind::try_from(hello.kind) {
        Ok(ConnectionKind::Control | ConnectionKind::Stream) => Ok(()),
        Ok(ConnectionKind::Input) => {
            Err("this daemon serves control and stream connections only; input connections \
             arrive in a later version"
                .to_string())
        }
        Ok(ConnectionKind::Unspecified) | Err(_) => {
            Err("the hello names no kind of connection this daemon knows".to_string())
        }
    }
}
