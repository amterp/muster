//! Which daemons a client may talk to.
//!
//! The rule is in the schema's header: the major must match, and the minor says which requests
//! and events a daemon knows. This is that rule in code, so the daemon refusing a handshake and a
//! client deciding whether to adopt a daemon cannot disagree about it.

use crate::Version;

/// The protocol this build speaks.
///
/// Bump `minor` with any change to the schema once a release has shipped the daemon, and `major`
/// when a message stops meaning what it meant. `proto/muster_daemon.proto`'s header says how the
/// baseline follows, and the `compatible` test holds the two together.
pub const PROTOCOL: Version = Version { major: 1, minor: 6 };

/// The first minor whose daemon presses the keys a `pane send` names. An older one ignores the
/// field and would write the send's Return without them, so a client asks before sending any.
pub const KEYS_IN_A_SEND: u32 = 4;

/// The first minor whose daemon keeps a tab hidden and a pane quiet. An older one drops both
/// fields, making a hidden pane visible and a quiet one ask, so a client asks before sending any.
pub const HIDDEN_AND_QUIET: u32 = 5;

/// The refusal for asking `daemon` to do `what` when it greeted with `speaks`, which is older
/// than `since`, the first minor that does it; nothing when it is not. A greeting that named no
/// protocol is read as the oldest there is. `done` says what was therefore not done.
///
/// An older daemon drops a field it does not know without a word, so a client asks this before
/// sending anything that needs one, rather than finding out from what went wrong.
pub fn too_old(
    daemon: &str,
    speaks: Option<&Version>,
    since: u32,
    what: &str,
    done: &str,
) -> Option<String> {
    let speaks = speaks.copied().unwrap_or_default();
    (speaks.minor < since).then(|| {
        format!(
            "{daemon} speaks protocol {speaks}, which cannot {what}, so nothing was {done}. \
             Update Muster; a new daemon takes over from an old one when the app starts."
        )
    })
}

/// Whether two ends speaking these versions can talk.
pub fn compatible(ours: &Version, theirs: &Version) -> bool {
    ours.major == theirs.major
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_different_minor_is_compatible_and_a_different_major_is_not() {
        let newer_minor = Version { major: PROTOCOL.major, minor: PROTOCOL.minor + 1 };
        let next_major = Version { major: PROTOCOL.major + 1, minor: 0 };
        assert!(compatible(&PROTOCOL, &newer_minor));
        assert!(compatible(&newer_minor, &PROTOCOL));
        assert!(!compatible(&PROTOCOL, &next_major));
    }

    #[test]
    fn a_daemon_older_than_a_feature_is_refused_it_and_one_naming_no_protocol_is_oldest() {
        let older = Version { major: 1, minor: HIDDEN_AND_QUIET - 1 };
        let said = too_old("the daemon", Some(&older), HIDDEN_AND_QUIET, "hide a pane", "changed");
        assert_eq!(
            said.as_deref(),
            Some(
                "the daemon speaks protocol 1.4, which cannot hide a pane, so nothing was \
                 changed. Update Muster; a new daemon takes over from an old one when the app \
                 starts."
            )
        );
        assert!(too_old("d", Some(&PROTOCOL), HIDDEN_AND_QUIET, "hide", "changed").is_none());
        assert!(too_old("d", None, KEYS_IN_A_SEND, "press keys", "sent").is_some());
    }
}
