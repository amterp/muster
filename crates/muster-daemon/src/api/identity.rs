//! A daemon's identity: a name drawn the first time a daemon serves its socket, and kept beside
//! it for every later run, a successor's included (MIP-10, section 3).
//!
//! Beside the socket because the socket is what makes a daemon this install's: a restart and a
//! handoff serve the same socket, and a test's scratch daemon has a socket and an identity of its
//! own. Not in the state file, which a daemon moves aside or stops writing when a restore goes
//! wrong, since an identity has to outlive both.

use std::path::{Path, PathBuf};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::names::Minter;

/// The file beside a daemon's socket: `~/.muster/daemon/<install>.identity`.
pub(crate) fn path_for(socket: &Path) -> PathBuf {
    socket.with_extension("identity")
}

/// This daemon's identity: the one written beside `socket`, or a new one written there.
pub(crate) fn of(socket: &Path) -> String {
    let path = path_for(socket);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let identity = text.trim();
            if is_identity(identity) {
                return identity.to_string();
            }
            log::warn(
                "daemon.identity.garbled",
                fields! {
                    "path" => path.display(),
                    "impact" => "a new identity replaces it, so programs using the API see this \
                                 daemon as a machine they have not met",
                    "check" => "what wrote to the file; only the daemon does",
                },
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            log::warn(
                "daemon.identity.unreadable",
                fields! {
                    "path" => path.display(),
                    "error" => error,
                    "impact" => "a new identity replaces it, so programs using the API see this \
                                 daemon as a machine they have not met",
                    "check" => "the file's permissions",
                },
            );
        }
    }
    let identity = Minter::default().daemon();
    if let Err(error) = write(&path, &identity) {
        log::warn(
            "daemon.identity.not_written",
            fields! {
                "path" => path.display(),
                "error" => error,
                "impact" => "this run's identity is kept for this run alone; the next run draws \
                             another, and programs using the API see it as a new machine",
                "check" => "the permissions on the socket's directory",
            },
        );
    }
    identity
}

/// Whether `text` is a name [`Minter::daemon`] could have drawn.
fn is_identity(text: &str) -> bool {
    text.len() > 1
        && text.starts_with('d')
        && text.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Writes `identity` in one rename, so a daemon that dies part way leaves the file whole or
/// absent.
fn write(path: &Path, identity: &str) -> std::io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, format!("{identity}\n"))?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drawn_identity_is_one_and_anything_else_is_not() {
        assert!(is_identity(&Minter::default().daemon()));
        for text in ["", "d", "p1w3r07bsd", "d1W3R07BSD", "d1w3 r07bsd"] {
            assert!(!is_identity(text), "{text:?}");
        }
    }
}
