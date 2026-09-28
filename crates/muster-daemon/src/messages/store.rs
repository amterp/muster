//! Where the message service's state lives on disk: beside the socket, in a directory named for
//! it, holding one append-only log per group and one file for everything else (MIP-4, section
//! 12). Both are readable by this user only, since a log holds whatever agents wrote.

use std::collections::BTreeMap;
use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_msg::{Entry, Saved, Store};
use serde::{Deserialize, Serialize};

use crate::persist;

/// The state file's format. A daemon refuses a newer one rather than dropping what it cannot
/// read, as it does for its tabs.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Kept {
    version: u32,
    #[serde(flatten)]
    saved: Saved,
}

#[derive(Debug)]
pub(crate) struct Files {
    directory: PathBuf,
}

/// What a daemon found in its message store.
pub(crate) struct Found {
    pub(crate) saved: Saved,
    pub(crate) logs: BTreeMap<String, Vec<Entry>>,
}

impl Files {
    pub(crate) fn beside(socket: &Path) -> Files {
        Files { directory: socket.with_extension("msg") }
    }

    fn state(&self) -> PathBuf {
        self.directory.join("state.json")
    }

    fn logs(&self) -> PathBuf {
        self.directory.join("groups")
    }

    fn log_of(&self, group: &str) -> PathBuf {
        self.logs().join(format!("{group}.log"))
    }

    fn ensure_directories(&self) -> std::io::Result<()> {
        DirBuilder::new().recursive(true).mode(0o700).create(self.logs())
    }

    /// Everything kept, or nothing where nothing was. A file that cannot be read is logged and
    /// left in place; it costs what it held, never the rest.
    pub(crate) fn load(&self) -> Found {
        let saved = match std::fs::read(self.state()) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Saved::default(),
            Err(error) => {
                unreadable(&self.state(), &error.to_string());
                Saved::default()
            }
            Ok(bytes) => match serde_json::from_slice::<Kept>(&bytes) {
                Ok(kept) if kept.version <= VERSION => kept.saved,
                Ok(kept) => {
                    unreadable(
                        &self.state(),
                        &format!(
                            "it is version {}, newer than this daemon's {VERSION}",
                            kept.version
                        ),
                    );
                    Saved::default()
                }
                Err(error) => {
                    unreadable(&self.state(), &error.to_string());
                    Saved::default()
                }
            },
        };
        let mut logs = BTreeMap::new();
        let Ok(files) = std::fs::read_dir(self.logs()) else {
            return Found { saved, logs };
        };
        for file in files.filter_map(Result::ok) {
            let path = file.path();
            if path.extension().is_none_or(|extension| extension != "log") {
                continue;
            }
            let Some(group) = path.file_stem().map(|stem| stem.to_string_lossy().into_owned())
            else {
                continue;
            };
            mend(&path);
            logs.insert(group, read_log(&path));
        }
        Found { saved, logs }
    }
}

impl Store for Files {
    /// One line per entry, synced before the post is answered: a message the daemon said it
    /// took survives the machine going down right after.
    fn append(&mut self, group: &str, entry: &Entry) -> Result<(), String> {
        let describe = |error: std::io::Error| format!("{}: {error}", self.log_of(group).display());
        self.ensure_directories().map_err(describe)?;
        let mut line = serde_json::to_vec(entry).map_err(|error| error.to_string())?;
        line.push(b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.log_of(group))
            .map_err(describe)?;
        let before = file.metadata().map_err(describe)?.len();
        let written = file.write_all(&line).and_then(|()| file.sync_data());
        if let Err(error) = written {
            // A fragment left here would join the next entry's line, and cost it too.
            let _ = file.set_len(before);
            return Err(describe(error));
        }
        Ok(())
    }

    fn save(&mut self, saved: &Saved) -> Result<(), String> {
        let describe = |error: std::io::Error| format!("{}: {error}", self.state().display());
        self.ensure_directories().map_err(describe)?;
        let kept = Kept { version: VERSION, saved: saved.clone() };
        let bytes = serde_json::to_vec_pretty(&kept).map_err(|error| error.to_string())?;
        persist::write(&self.state(), &bytes).map_err(describe)
    }
}

/// Cuts a log back to its last complete line. A last line with no newline is what a crash during
/// an append leaves - a post that was never answered - and the next append would otherwise be
/// written onto the end of it.
fn mend(path: &Path) {
    let Ok(bytes) = std::fs::read(path) else { return };
    if bytes.last().is_none_or(|last| *last == b'\n') {
        return;
    }
    let whole = bytes.iter().rposition(|byte| *byte == b'\n').map_or(0, |at| at + 1);
    let cut = OpenOptions::new().write(true).open(path).and_then(|file| {
        file.set_len(whole as u64)?;
        file.sync_data()
    });
    match cut {
        Ok(()) => log::warn(
            "msg.store.torn_line_dropped",
            fields! {
                "file" => path.display(),
                "bytes" => bytes.len() - whole,
                "impact" => "none: it was a post the daemon never answered, because it went \
                             down while writing it",
                "check" => "whether the daemon or the machine went down around then",
            },
        ),
        Err(error) => unreadable(path, &format!("its torn last line could not be cut: {error}")),
    }
}

/// A group's entries. A line that does not parse is skipped and logged: the last one is what a
/// crash mid-append leaves, and anything else is damage that should not cost the rest.
fn read_log(path: &Path) -> Vec<Entry> {
    let text = match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => {
            unreadable(path, &error.to_string());
            return Vec::new();
        }
    };
    let mut entries = Vec::new();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Entry>(line) {
            Ok(entry) => entries.push(entry),
            Err(error) => log::warn(
                "msg.store.line_skipped",
                fields! {
                    "file" => path.display(),
                    "line" => number + 1,
                    "error" => error,
                    "impact" => "that entry is missing from the group's log and from every read",
                    "check" => "a last line cut short is what a crash during a post leaves; \
                                a line anywhere else means the file was edited or damaged",
                },
            ),
        }
    }
    entries
}

fn unreadable(path: &Path, error: &str) {
    log::error(
        "msg.store.unreadable",
        fields! {
            "file" => path.display(),
            "error" => error,
            "impact" => "the daemon starts without what the file held: participants lose their \
                         read cursors, or a group its log",
            "check" => "the file's permissions and contents; it is left where it is",
        },
    );
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use muster_msg::What;

    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("muster-msg-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path.join("daemon.sock")
    }

    fn message(seq: u64, body: &str) -> Entry {
        Entry {
            seq,
            at_ms: seq,
            what: What::Message { author: "a".to_string(), to: Vec::new(), body: body.to_string() },
        }
    }

    #[test]
    fn what_was_appended_and_saved_is_what_loads() {
        let socket = scratch("round-trip");
        let mut files = Files::beside(&socket);
        files.append("g", &message(1, "one")).unwrap();
        files.append("a+b", &message(1, "two")).unwrap();
        let saved = Saved { participants: Vec::new(), groups: Vec::new() };
        files.save(&saved).unwrap();

        let found = Files::beside(&socket).load();
        assert_eq!(found.logs["g"], vec![message(1, "one")]);
        assert_eq!(found.logs["a+b"], vec![message(1, "two")]);
        assert_eq!(found.saved, saved);
        let mode = std::fs::metadata(socket.with_extension("msg")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn a_line_cut_short_by_a_crash_costs_only_itself() {
        let socket = scratch("torn");
        let mut files = Files::beside(&socket);
        files.append("g", &message(1, "kept")).unwrap();
        let mut file = OpenOptions::new().append(true).open(files.log_of("g")).unwrap();
        file.write_all(b"{\"seq\":2,\"at_ms\":2,\"kind\":\"mess").unwrap();
        assert_eq!(Files::beside(&socket).load().logs["g"], vec![message(1, "kept")]);
    }

    /// The next post after a crash starts on a line of its own, so the entry the daemon answered
    /// as kept is still there after the restart after that.
    #[test]
    fn an_entry_appended_after_a_torn_line_is_kept() {
        let socket = scratch("torn-then-appended");
        let mut files = Files::beside(&socket);
        files.append("g", &message(1, "kept")).unwrap();
        let mut file = OpenOptions::new().append(true).open(files.log_of("g")).unwrap();
        file.write_all(b"{\"seq\":2,\"at_ms\":2,\"kind\":\"mess").unwrap();

        let mut files = Files::beside(&socket);
        assert_eq!(files.load().logs["g"], vec![message(1, "kept")]);
        files.append("g", &message(2, "answered as kept")).unwrap();
        assert_eq!(
            Files::beside(&socket).load().logs["g"],
            vec![message(1, "kept"), message(2, "answered as kept")]
        );
    }

    #[test]
    fn a_state_file_from_a_newer_daemon_is_not_read() {
        let socket = scratch("newer");
        let files = Files::beside(&socket);
        files.ensure_directories().unwrap();
        std::fs::write(files.state(), br#"{"version":99,"participants":[],"groups":[]}"#).unwrap();
        assert_eq!(Files::beside(&socket).load().saved, Saved::default());
        assert!(files.state().exists(), "the newer file is left for the newer daemon");
    }
}
