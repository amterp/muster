//! What a daemon record says, and the rules that keep the directory readable.
//!
//! The file's meaning lives here and the directory lives in the herdr adapter, the same split
//! `names.rs` and `shared_names.rs` draw. What is worth pinning at this level is the three
//! things a reader depends on: a record round-trips, a version this build does not know is
//! skipped rather than guessed at, and the bound drops the right records.

use muster_core::daemons::{KEPT, Started, beyond_the_bound, from_toml, holding, to_toml};

#[test]
fn a_record_says_the_same_thing_after_a_round_trip() {
    let written = Started {
        socket: "/Users/you/.config/herdr/sessions/muster/herdr.sock".to_string(),
        started: 1_756_000_000,
    };

    assert_eq!(from_toml(&to_toml(&written)).as_ref(), Some(&written));
}

#[test]
fn a_record_this_build_does_not_understand_is_left_out_rather_than_guessed_at() {
    // The same terms as the saved arrangement and the name registry: a file from a Muster that
    // writes more than this one knows how to read is skipped. What that costs is one row in a
    // census; what guessing would cost is a row saying something wrong about a process
    // somebody is deciding whether to end.
    let ahead = "version = 99\nsocket = \"/tmp/a/herdr.sock\"\nstarted = 1\n";
    let nonsense = "this is not toml at all {{{";
    let nameless = "version = 1\nsocket = \"\"\nstarted = 1\n";

    assert_eq!(from_toml(ahead), None);
    assert_eq!(from_toml(nonsense), None);
    assert_eq!(from_toml(nameless), None, "a record with no socket names nothing to check");
}

#[test]
fn a_record_with_no_time_in_it_still_names_its_socket() {
    // A time is what a reader recognises a daemon by, and nothing acts on it - so a record
    // missing one is still the useful half. Dropping it would lose the socket, which is the
    // only thing anything can do anything with.
    let record = "version = 1\nsocket = \"/tmp/a/herdr.sock\"\n";

    let read = from_toml(record).expect("a record naming a socket is readable");
    assert_eq!(read.socket, "/tmp/a/herdr.sock");
    assert_eq!(read.started, 0);
}

#[test]
fn a_daemon_restarted_on_one_socket_replaces_its_own_record() {
    // The socket is the identity. Without this a machine whose daemon has been restarted a
    // hundred times has a hundred rows for one daemon, and the census it exists to make
    // readable is the thing it made unreadable.
    let records = vec![
        ("daemon-1.toml", started("/tmp/a/herdr.sock", 10)),
        ("daemon-2.toml", started("/tmp/b/herdr.sock", 20)),
    ];

    assert_eq!(holding(&records, "/tmp/b/herdr.sock"), Some(&"daemon-2.toml"));
    assert_eq!(holding(&records, "/tmp/c/herdr.sock"), None);
}

#[test]
fn the_bound_drops_the_oldest_and_only_when_one_more_would_not_fit() {
    let under: Vec<(u32, Started)> =
        (0..kept() - 1).map(|n| (n, started(&format!("/tmp/{n}/herdr.sock"), n.into()))).collect();
    assert!(beyond_the_bound(&under).is_empty(), "a directory with room takes another record");

    let full: Vec<(u32, Started)> =
        (0..kept()).map(|n| (n, started(&format!("/tmp/{n}/herdr.sock"), n.into()))).collect();
    assert_eq!(
        beyond_the_bound(&full),
        vec![0],
        "a full directory drops its oldest record, and exactly one of them"
    );

    // By what the record says, not by the file. A daemon restarted on an old socket rewrites
    // its record, which touches the file - so a rule that read the filesystem would call the
    // oldest daemon on the machine the newest thing in the directory.
    let mut jumbled = full;
    jumbled.reverse();
    assert_eq!(beyond_the_bound(&jumbled), vec![0]);
}

fn started(socket: &str, at: u64) -> Started {
    Started { socket: socket.to_string(), started: at }
}

/// The bound as the width the ids in these cases are counted at.
fn kept() -> u32 {
    u32::try_from(KEPT).expect("the bound is a small number")
}
