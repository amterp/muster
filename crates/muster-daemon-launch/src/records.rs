//! Writing down the daemons Muster starts, one file per daemon under the directory the shell
//! names (`~/.muster/state/daemons`). Reading them back and dialing each is the census, in
//! `muster-daemon-client`, since asking a daemon what it holds needs a control connection.
//!
//! `muster_core::daemons` owns what a record says; this owns the directory it lives in.
//!
//! **One file per daemon, keyed by socket, so nothing needs a lock.** Two windows starting
//! daemons start them on different sockets - a second window on the same socket adopts rather
//! than starts - so no two writers ever reach for one file. That is what lets this be a plain
//! write where the shared name registry needs a hold.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use muster_core::daemons::{Started, beyond_the_bound, from_toml, holding, to_toml};
use muster_core::diagnostics::log;
use muster_core::fields;

/// Writes down a daemon Muster has just started.
///
/// Called only for a daemon Muster started, never for one it adopted. Muster can vouch for what
/// it started; a daemon that was already answering belongs to whoever started it, and claiming
/// it in this census would offer somebody a process to end on Muster's word.
///
/// Failures are logged and swallowed. A window whose daemon started is a working window, and
/// refusing to open it because a record could not be written would trade the feature for the
/// thing the feature is about.
pub fn started(directory: &str, socket: &str) {
    let directory = Path::new(directory);
    if let Err(error) = std::fs::create_dir_all(directory) {
        return complain("daemons.record.unwritable", socket, &error.to_string());
    }

    let existing = read_directory(directory);
    let record = Started { socket: socket.to_string(), started: now() };
    // A daemon restarted on the same socket replaces its own row rather than adding one.
    let path = if let Some(path) = holding(&existing, socket) {
        path.clone()
    } else {
        for stale in beyond_the_bound(&existing) {
            let _ = std::fs::remove_file(&stale);
        }
        mint(directory, &existing)
    };
    if let Err(error) = std::fs::write(&path, to_toml(&record)) {
        complain("daemons.record.unwritable", socket, &error.to_string());
    }
}

/// Every readable record in the directory, paired with the file it came from.
pub fn read_directory(directory: &Path) -> Vec<(PathBuf, Started)> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    let mut found: Vec<(PathBuf, Started)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "toml"))
        .filter_map(|path| {
            let record = std::fs::read_to_string(&path).ok()?;
            from_toml(&record).map(|started| (path, started))
        })
        .collect();
    // Settled, so two runs of a census over an unchanged directory read the same - a directory
    // hands its entries back in whatever order it likes.
    found.sort_by(|(left, _), (right, _)| left.cmp(right));
    found
}

/// A file name nothing is using, numbered the way the saved arrangements are.
///
/// Numbered rather than derived from the socket, which is the obvious alternative and needs an
/// encoding: a socket path is not a file name, and two paths that differ only in a separator
/// would collide under any cheap flattening of one. The socket lives inside the file, which is
/// where anything looking for it reads it.
fn mint(directory: &Path, existing: &[(PathBuf, Started)]) -> PathBuf {
    let taken: Vec<&PathBuf> = existing.iter().map(|(path, _)| path).collect();
    let mut number = 1;
    loop {
        let candidate = directory.join(format!("daemon-{number}.toml"));
        if !taken.contains(&&candidate) && !candidate.exists() {
            return candidate;
        }
        number += 1;
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_secs()).unwrap_or_default()
}

fn complain(event: &str, socket: &str, detail: &str) {
    log::warn(
        event,
        fields! {
            "socket" => socket,
            "detail" => detail,
            "impact" => "this daemon is missing from `muster daemons`, so somebody deciding \
                         which daemon process to end is not shown one Muster started - and it \
                         is the census that makes ending one safe",
            "check" => "whether ~/.muster/state/daemons is writable; the window itself is \
                        unaffected and its panes work normally",
        },
    );
}
