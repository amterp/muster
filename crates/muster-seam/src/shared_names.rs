//! The file two Musters name things in, and the lock that stops them doing it at once.
//!
//! `panes.toml` was a file one window wrote and the next one read. A second window makes it
//! something else: both are attached to the same daemon, both see every pane it holds, and both
//! would name a pane nobody had named yet. Measured before this existed, two windows on one
//! daemon agreed about exactly one pane - the one that was already there when the second opened -
//! and the window that saved last took the other's bindings with it.
//!
//! So the core reads, names and writes inside one hold, and this is the hold. Everything about
//! where the file is and how a lock is taken is here, because those are OS questions and the
//! core has no business with either (`architecture.md`, the shell/core seam).

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::names::SharedNames;

/// The record, as a path and the lock beside it.
#[derive(Debug)]
pub(crate) struct NamesFile {
    record: PathBuf,
    /// A file of its own rather than the record, because the record is replaced by a rename
    /// every time it is written - and a lock held on an inode that has just been renamed out of
    /// the way is a lock two writers can both believe they hold.
    lock: PathBuf,

    /// What the record looked like when this Muster last had it open.
    ///
    /// Compared rather than remembered as a flag, so that a window which was never told
    /// anything and a window whose record has not moved answer the same way. Zero is "never
    /// read it", which reads as moved and costs one hold on the first naming.
    seen: AtomicU64,
}

impl NamesFile {
    pub(crate) fn at(path: &str) -> NamesFile {
        NamesFile {
            record: PathBuf::from(path),
            lock: PathBuf::from(format!("{path}.lock")),
            seen: AtomicU64::new(0),
        }
    }
}

impl SharedNames for NamesFile {
    fn moved(&self) -> bool {
        self.seen.load(Ordering::Relaxed) != self.revision()
    }

    fn exclusively(&self, while_held: &mut dyn FnMut(&str) -> Option<String>) {
        // Held for the whole of this function and released by dropping, including on the paths
        // that give up early - which is why the guard is bound rather than matched on.
        let held = Hold::take(&self.lock);
        if held.is_none() {
            // Named once rather than per call: a directory that cannot be written to will not
            // start being writable, and a line per publish would bury the run log.
            log::warn(
                "names.unlocked",
                fields! {
                    "path" => self.lock.display().to_string(),
                    "impact" => "names are still written, and a second Muster naming the same \
                                 pane at the same moment could win the race - the two windows \
                                 would then call one pane two things",
                    "check" => "whether that directory exists and is writable",
                },
            );
        }

        let record = std::fs::read_to_string(&self.record).unwrap_or_default();
        let Some(written) = while_held(&record) else {
            self.remember();
            return;
        };
        if written == record {
            self.remember();
            return;
        }

        // Staged and renamed, so a window killed mid-write leaves the record it had rather than
        // half of one - which would strand every pane after the line it stopped on.
        let staged = self.record.with_extension("writing");
        let saved = std::fs::create_dir_all(self.record.parent().unwrap_or_else(|| Path::new(".")))
            .and_then(|()| std::fs::write(&staged, &written))
            .and_then(|()| std::fs::rename(&staged, &self.record));

        // After the write, so that this window does not read its own change as somebody
        // else's and take the hold again on the very next naming.
        self.remember();

        if let Err(error) = saved {
            log::warn(
                "names.save.failed",
                fields! {
                    "path" => self.record.display().to_string(),
                    "detail" => error.to_string(),
                    "impact" => "these names last until this Muster quits. Every pane open at \
                                 that moment keeps a name in its environment that the next \
                                 launch will not know, so commands from inside them are \
                                 refused - and the next launch cannot find the tabs its saved \
                                 arrangement names, so it opens fresh",
                    "check" => "whether that directory exists and is writable",
                },
            );
        }
    }
}

impl NamesFile {
    /// What the record is at this moment, as one number.
    ///
    /// Modification time and size together rather than either alone: a record rewritten within
    /// a clock tick usually changes length, and one that keeps its length usually moves in
    /// time. Neither is a guarantee, and neither has to be - being wrong here costs one window
    /// one stale name until the next write, where reading the whole file on every naming would
    /// cost a lock per keystroke.
    ///
    /// Zero for a record that is not there yet, which reads as moved: the first naming then
    /// takes the hold, which is what it would have to do anyway.
    fn revision(&self) -> u64 {
        let Ok(about) = std::fs::metadata(&self.record) else { return 0 };
        // Truncating on purpose, and harmless: the low 64 bits of a nanosecond count roll over
        // once every 584 years, and what is being asked is only "is this the same file I read".
        let moved = about
            .modified()
            .ok()
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| u64::try_from(since.as_nanos() & u128::from(u64::MAX)).unwrap_or(0));
        moved ^ about.len().rotate_left(32)
    }

    fn remember(&self) {
        self.seen.store(self.revision(), Ordering::Relaxed);
    }
}

/// An exclusive lock on a file, for as long as this is alive.
///
/// `flock` rather than a lock file somebody has to remember to delete: the kernel drops it when
/// the descriptor closes, so a Muster that crashes holding it blocks nobody. Advisory, which is
/// all that is wanted - every writer of this record is a Muster and every one of them takes it.
#[derive(Debug)]
struct Hold(File);

impl Hold {
    fn take(path: &Path) -> Option<Hold> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory).ok()?;
        }
        let file = OpenOptions::new().create(true).write(true).truncate(false).open(path).ok()?;
        // Blocking on purpose. The other holder is another Muster naming one thing, which is a
        // file read and a file write - and waiting for that is the whole point. `LOCK_NB` here
        // would mean carrying on without the lock, which is the race this exists to close.
        //
        // SAFETY: the descriptor is open for the life of `file`, which this owns.
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        (locked == 0).then_some(Hold(file))
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        // SAFETY: as above - still open, because dropping the file happens after this.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Inline rather than in `tests/`, because what is being tested is this file's own hold and
/// `NamesFile` is not public. Two registries over one record is what two Musters are, and the
/// only thing standing in for a second process is the second registry.
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use muster_core::mirror::backend::PaneId;
    use muster_core::names::{Mint, Names, PaneNames, SharedNames, TabNames};

    use super::NamesFile;

    /// A window, as far as naming is concerned.
    ///
    /// Its own registries *and* its own `NamesFile` over the same path, because that is what a
    /// second window is: another process, holding its own idea of what the record last said.
    /// Sharing one `NamesFile` between the two would share the very thing that tells a window
    /// the record has moved, and the test would pass without proving anything.
    fn window(at: &std::path::Path, daemon: &str) -> Names {
        Names::sharing(
            muster_core::composition::DaemonId::new(daemon),
            Arc::new(Mutex::new(PaneNames::new(Mint::Drawn))),
            Arc::new(Mutex::new(TabNames::new(Mint::Drawn))),
            Arc::new(NamesFile::at(&at.to_string_lossy())) as Arc<dyn SharedNames>,
        )
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let at = std::env::temp_dir().join(format!("muster-names-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&at);
        std::fs::create_dir_all(&at).expect("a scratch directory can be made");
        at.join("panes.toml")
    }

    /// Two windows meeting the same pane call it the same thing.
    ///
    /// The failure this exists for was measured rather than imagined: before the record was
    /// held across naming, two Musters on one daemon agreed about exactly one pane - the one
    /// that existed before the second window opened - and every pane made afterwards had two
    /// names, one per window. Every `muster --socket` command that named a pane then reached
    /// the wrong window's idea of it.
    #[test]
    fn two_windows_naming_one_pane_agree_about_it() {
        let at = scratch("agree");
        let (first, second) = (window(&at, "local"), window(&at, "local"));

        let theirs = second.pane("w1:p7");
        let ours = first.pane("w1:p7");

        assert_eq!(
            ours, theirs,
            "two windows named one pane two things, which is what makes `muster --socket` \
             unable to address a pane across windows"
        );
    }

    /// A window naming a pane does not lose the names another window already wrote.
    ///
    /// The second half of the same failure: the record was written whole from memory, so
    /// whichever window saved last replaced the other's bindings - and the next launch resolved
    /// only the survivor's, leaving the other window's saved arrangement naming tabs nothing
    /// knew.
    #[test]
    fn naming_keeps_what_another_window_wrote() {
        let at = scratch("keep");
        let (first, second) = (window(&at, "local"), window(&at, "local"));

        let theirs = second.pane("w1:p1");
        let ours = first.pane("w1:p2");

        let written = std::fs::read_to_string(&at).expect("the record is written");
        for name in [&theirs, &ours] {
            assert!(
                written.contains(&name.to_string()),
                "{name} is not in the record after the other window wrote to it:\n{written}"
            );
        }
        assert_eq!(second.pane("w1:p2"), ours, "the other window did not take on the new name");
    }

    /// A name a window reserved is the one that survives, because a pane is already holding it.
    ///
    /// `MUSTER_PANE` is set in the request that creates a pane, so a reserved name is spoken
    /// before anything can be written down. Another window that met the pane first and named it
    /// has to give way, or a `muster` run inside that pane names something no window agrees is
    /// there.
    #[test]
    fn a_reserved_name_beats_one_another_window_guessed() {
        let at = scratch("reserved");
        let (maker, other) = (window(&at, "local"), window(&at, "local"));

        let reserved: PaneId = maker.reserve();
        let guessed = other.pane("w1:p4");
        assert_ne!(reserved, guessed, "the two started out agreeing, so this proves nothing");

        maker.settle(&reserved, "w1:p4");

        assert_eq!(
            other.pane("w1:p4"),
            reserved,
            "the window that did not make the pane kept its own guess, so the name in that \
             pane's environment resolves nowhere over there"
        );
    }
}
