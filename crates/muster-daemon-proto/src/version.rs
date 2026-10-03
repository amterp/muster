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
pub const PROTOCOL: Version = Version { major: 1, minor: 3 };

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
}
