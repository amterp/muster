//! Reading the recorded corpus, and comparing against a checked-in rendering.
//!
//! Both live in `corpus/`, resolved by walking up from this crate rather than through any
//! build system's resource copying: reading a case and running it should be the same file,
//! and a copy is a thing that can go stale.

// Each integration test is its own binary and compiles this module separately, so a helper
// only one of them needs reads as dead code in the others.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Bytes of a file under `corpus/`.
pub(crate) fn corpus_file(path: &str) -> Vec<u8> {
    let full = corpus_dir().join(path);
    std::fs::read(&full)
        .unwrap_or_else(|error| panic!("corpus file {} is unreadable: {error}", full.display()))
}

pub(crate) fn corpus_text(path: &str) -> String {
    String::from_utf8(corpus_file(path)).expect("this corpus file should be UTF-8")
}

/// Compares against a checked-in file, or writes it when asked.
///
/// `MUSTER_UPDATE_SNAPSHOTS=1` rewrites and then fails, so a recording run cannot pass for
/// a real one: a snapshot tool that makes accepting a change effortless eventually records
/// a bug as the expectation.
pub(crate) fn expect_snapshot(actual: &str, name: &str) {
    let path = corpus_dir().join("snapshots").join(name);

    if std::env::var("MUSTER_UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
        std::fs::write(&path, actual).expect("the snapshot directory should be writable");
        panic!(
            "Recorded snapshot {name}. This run proves nothing: MUSTER_UPDATE_SNAPSHOTS was \
             set, so every case wrote its own expectation. Review the diff, then re-run \
             without it."
        );
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "No snapshot at {}. Nothing was verified for this case. Create it with \
             MUSTER_UPDATE_SNAPSHOTS=1 and read the file it writes before committing it.",
            path.display()
        )
    });

    if expected == actual {
        return;
    }

    // A whole-file dump of an 80x24 grid buries the one row that changed, which is the only
    // thing the reader needs.
    let difference = expected
        .lines()
        .zip(actual.lines())
        .enumerate()
        .find(|(_, (want, got))| want != got)
        .map_or_else(
            || "Files differ only in trailing content.".to_string(),
            |(index, (want, got))| {
                format!(
                    "First difference at line {}:\n  expected: {want:?}\n  actual:   {got:?}",
                    index + 1
                )
            },
        );

    panic!(
        "\nSnapshot {name} does not match.\n{difference}\nIf the new output is right, \
         re-record with MUSTER_UPDATE_SNAPSHOTS=1 and review the diff. If it is not, this is \
         the bug the snapshot was there to catch.\n"
    );
}

fn corpus_dir() -> PathBuf {
    let mut directory: &Path = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = directory.join("corpus");
        if candidate.is_dir() {
            return candidate;
        }
        directory = directory.parent().expect("corpus/ should be somewhere above this crate");
    }
}
