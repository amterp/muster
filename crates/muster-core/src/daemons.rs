//! What Muster wrote down about a daemon it started, so a later launch can find it again.
//!
//! `muster window` says which daemons *this* window is attached to. It cannot say which
//! daemons are on this machine that nothing is attached to, and those are the ones that
//! accumulate: twenty alive on one machine, nineteen holding nothing, one holding somebody's
//! work - and the live one was neither the oldest nor the youngest, so age picks wrong
//! (kan a_2InphPbdQ).
//!
//! `./dev --doctor` answers it by walking processes to sockets with `pgrep` and `lsof`. That
//! cannot be the answer here: it is two Unix tools in a build script, and this core is
//! portable by construction. What Muster can do instead is write down what it started, which
//! nothing else knows - herdr has no method answering "which process are you", so the pairing
//! of a daemon with the work inside it exists only where Muster put it.
//!
//! **A record is a hint that gets checked, never an answer.** It says a daemon was started on
//! this socket; whether one is there now is settled by dialing, and a record that names a
//! socket nothing answers on cannot tell a daemon that exited from one that has gone
//! unreachable. So nothing here concludes anything about a daemon's liveness - it carries the
//! socket, and [`crate::intent`]'s adapter asks.
//!
//! **What this must never become is a reaper.** A process holding somebody's live agent is the
//! wrong thing to end on a schedule, in a tool whose whole promise is that agents outlive the
//! app. The record exists so that ending one is deliberate, and refusing to automate that is
//! the point rather than an omission (kan a_28YghIUw2).

use crate::diagnostics::log;
use crate::fields;

/// The version this format is on.
///
/// A file this Muster does not understand is skipped rather than guessed at, the same terms as
/// the saved arrangement and the name registry. What that costs is one daemon missing from a
/// census, which is a smaller loss than a census that says something wrong.
const VERSION: i64 = 1;

/// How many daemons are worth remembering.
///
/// A record is a few hundred bytes, so this is about a directory somebody opens rather than
/// about space - and about a census staying readable, which is the whole point of it. The same
/// number the saved arrangements keep, for the same reason.
pub const KEPT: usize = 20;

/// One daemon Muster started, as the file remembers it.
///
/// Started rather than attached. Muster adopts a daemon that is already answering, and an
/// adopted one is somebody else's to account for - `muster window` names it while this window
/// is using it, and Muster has no standing to tell you what it holds after that.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Started {
    /// The socket it binds, which is the handle for everything anyone can do to it.
    ///
    /// Also the identity: two records naming one socket are one daemon written down twice, and
    /// a daemon restarted on the same socket replaces its own record rather than adding one.
    pub socket: String,

    /// When it was started, in seconds since the epoch. Zero when the record does not say.
    ///
    /// Not what decides anything. Age is exactly the thing that picked the wrong process on
    /// the machine this card was raised from, and it is here so a reader can recognise a
    /// daemon they remember starting rather than so anything can sort by it.
    pub started: u64,
}

/// A record as it goes into a file.
pub fn to_toml(started: &Started) -> String {
    let mut root = toml::Table::new();
    root.insert("version".to_string(), toml::Value::Integer(VERSION));
    root.insert("socket".to_string(), toml::Value::String(started.socket.clone()));
    root.insert(
        "started".to_string(),
        toml::Value::Integer(i64::try_from(started.started).unwrap_or(i64::MAX)),
    );
    toml::to_string_pretty(&toml::Value::Table(root))
        .unwrap_or_else(|error| panic!("a daemon record should always render as TOML: {error}"))
}

/// What one file says, or nothing when it says nothing usable.
///
/// `None` rather than an error, because every caller does the same thing about it: a record
/// that will not read is one daemon left out of a census, and a census that refused to answer
/// at all because one file was corrupt would be worse. The reason goes to the log, where
/// somebody chasing a daemon they cannot find will look.
pub fn from_toml(record: &str) -> Option<Started> {
    let table: toml::Table = match record.parse() {
        Ok(table) => table,
        Err(error) => {
            log::warn(
                "daemons.record.unreadable",
                fields! {
                    "detail" => error.to_string(),
                    "impact" => "one daemon Muster started is left out of `muster daemons`, so \
                                 somebody deciding what to end will not be shown it",
                    "check" => "the file under ~/.muster/state/daemons/; it is Muster's own and \
                                deleting it costs only this row",
                },
            );
            return None;
        }
    };
    if table.get("version").and_then(toml::Value::as_integer) != Some(VERSION) {
        return None;
    }
    let socket = table.get("socket")?.as_str()?.to_string();
    if socket.is_empty() {
        return None;
    }
    let started = table
        .get("started")
        .and_then(toml::Value::as_integer)
        .and_then(|seconds| u64::try_from(seconds).ok())
        .unwrap_or_default();
    Some(Started { socket, started })
}

/// Which of the records already written this socket belongs to, if any.
///
/// The socket is the identity, so a daemon restarted on the same path rewrites its own row
/// rather than adding a second. `records` is whatever was read out of the directory, paired
/// with wherever each one came from.
pub fn holding<'a, K>(records: &'a [(K, Started)], socket: &str) -> Option<&'a K> {
    records.iter().find(|(_, started)| started.socket == socket).map(|(where_it_is, _)| where_it_is)
}

/// Which records to drop so that writing one more leaves [`KEPT`] of them.
///
/// Oldest first, by what the record itself says rather than by when the file was touched: a
/// census is about daemons and not about files, and a record rewritten by a restart would
/// otherwise look like the newest thing in the directory.
///
/// Bounded on write rather than swept on read, so that reading a census changes nothing. A
/// read verb that quietly deleted records would be one nobody could run twice and compare.
pub fn beyond_the_bound<K: Clone>(records: &[(K, Started)]) -> Vec<K> {
    if records.len() < KEPT {
        return Vec::new();
    }
    let mut by_age: Vec<&(K, Started)> = records.iter().collect();
    by_age.sort_by_key(|(_, started)| started.started);
    by_age
        .into_iter()
        .take(records.len() + 1 - KEPT)
        .map(|(where_it_is, _)| where_it_is.clone())
        .collect()
}
