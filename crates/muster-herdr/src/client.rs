//! A request to a herdr daemon over its JSON socket.
//!
//! One connection per request, because that is what the daemon does: it reads a single
//! newline-terminated line, answers with one line, and closes. Holding a connection open
//! buys nothing for anything Muster sends today.
//!
//! Deliberately blocking. The one caller serializes its own sends, because two routes reach
//! the same PTY and order between them is the whole correctness question; an async client
//! here would hand that problem back.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

/// Why a request did not produce a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// No daemon answered - wrong path, not running, or gone.
    Unreachable(String),
    /// It answered too slowly, or stopped mid-answer.
    TimedOut,
    /// It answered with something that is not a herdr response.
    MalformedResponse,
    /// It answered, and the answer was no.
    Daemon { code: String, message: String },
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Unreachable(detail) => write!(f, "unreachable ({detail})"),
            Failure::TimedOut => f.write_str("timed out"),
            Failure::MalformedResponse => f.write_str("malformed response"),
            Failure::Daemon { code, message } => write!(f, "{code}: {message}"),
        }
    }
}

#[derive(Debug)]
pub struct HerdrClient {
    socket_path: String,
    timeout: Duration,
    next_id: AtomicU64,
}

impl HerdrClient {
    /// Half a second, both directions.
    ///
    /// Bounded because this sits on the input path: a wedged daemon must not take the
    /// keyboard with it, and a keystroke that falls back to a guessed encoding beats one
    /// that hangs the window.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);

    pub fn new(socket_path: impl Into<String>) -> HerdrClient {
        HerdrClient::with_timeout(socket_path, HerdrClient::DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(socket_path: impl Into<String>, timeout: Duration) -> HerdrClient {
        HerdrClient { socket_path: socket_path.into(), timeout, next_id: AtomicU64::new(0) }
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Sends one request and returns the `result` object.
    pub fn request(&self, method: &str, params: &Value) -> Result<Value, Failure> {
        self.request_within(method, params, self.timeout)
    }

    /// The same, for a call whose answer is worth waiting longer than a keystroke for.
    ///
    /// A timeout per call rather than per client, because each request opens a connection of
    /// its own anyway - so the bound belongs to the question being asked. The default is short
    /// on purpose (see [`DEFAULT_TIMEOUT`](HerdrClient::DEFAULT_TIMEOUT)); a call that is
    /// *waiting for something to happen* rather than asking what is true has no business being
    /// held to it.
    pub fn request_within(
        &self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, Failure> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let envelope = json!({ "id": format!("muster:{id}"), "method": method, "params": params });
        let mut payload = envelope.to_string().into_bytes();
        // The newline is not decoration: the daemon reads exactly one line and blocks
        // without it (src/api/server.rs, read_initial_request_line).
        payload.push(b'\n');

        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|error| Failure::Unreachable(error.to_string()))?;
        stream.set_read_timeout(Some(timeout)).map_err(|_| Failure::TimedOut)?;
        stream.set_write_timeout(Some(timeout)).map_err(|_| Failure::TimedOut)?;

        // Written and then left open. Half-closing the write side here is what this did until
        // the first call arrived that the daemon answers slowly: herdr reads its one request
        // line and does not need end-of-write, and for `pane.wait_for_output` it treats a
        // half-closed socket as a caller that has gone and hangs up without answering.
        stream.write_all(&payload).map_err(|_| Failure::TimedOut)?;

        let line = read_line(&mut stream).ok_or(Failure::TimedOut)?;
        let object: Value =
            serde_json::from_slice(&line).map_err(|_| Failure::MalformedResponse)?;

        if let Some(error) = object.get("error") {
            return Err(Failure::Daemon {
                code: error.get("code").and_then(Value::as_str).unwrap_or("unknown").to_string(),
                message: error.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
            });
        }
        Ok(object.get("result").cloned().unwrap_or_else(|| json!({})))
    }
}

/// Reads one newline-terminated response.
///
/// The cap only bounds a daemon that has started saying something unbounded; a real
/// response is small and arrives in one or two reads.
fn read_line(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut byte = [0u8; 1];
    while out.len() < 1 << 20 {
        match stream.read(&mut byte) {
            Ok(1) => {
                if byte[0] == b'\n' {
                    return Some(out);
                }
                out.push(byte[0]);
            }
            // A clean end without a newline still carries an answer worth parsing; only a
            // silent one is nothing.
            _ => return if out.is_empty() { None } else { Some(out) },
        }
    }
    Some(out)
}
