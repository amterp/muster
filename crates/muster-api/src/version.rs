//! Which clients a daemon serves.
//!
//! The rule is in the schema's header. This is that rule in code, so the daemon refusing a Hello
//! and a client reading the refusal cannot disagree about it.

use crate::Version;

/// The API this build speaks. Every change to the schema raises `minor`.
pub const API: Version = Version { major: 0, minor: 2 };

/// The oldest API a daemon of this build serves. Before 1.0, a change that breaks clients moves
/// it up to the new `API`; from 1.0 it is the major's first minor, since a break is a new major.
pub const SERVES_FROM: Version = Version { major: 0, minor: 1 };

// The oldest version served is never newer than this build, and from 1.0 is its major's first.
const _: () = {
    assert!(SERVES_FROM.major == API.major);
    assert!(SERVES_FROM.minor <= API.minor);
    assert!(API.major == 0 || SERVES_FROM.minor == 0);
};

/// Whether a daemon of this build serves a client that speaks `client`, and if not, why, in words
/// for the client's author.
pub fn serves(client: &Version) -> Result<(), String> {
    if client.major != API.major {
        return Err(format!(
            "this daemon speaks API {API} and the client speaks {client}; a client is served \
             only by a daemon of its own major version"
        ));
    }
    if client.minor < SERVES_FROM.minor {
        return Err(format!(
            "this daemon speaks API {API}, which changed incompatibly after {client}, and it \
             serves clients of {SERVES_FROM} or later; update the client"
        ));
    }
    Ok(())
}

/// Whether a client of this build can read a daemon that speaks `daemon`, and if not, why: the
/// client's half of the rule, since a daemon cannot know of a break made after it was built.
pub fn understands(daemon: &Version) -> Result<(), String> {
    if daemon.major != API.major {
        return Err(format!(
            "the daemon speaks API {daemon} and this client speaks {API}; a client reads only a \
             daemon of its own major version"
        ));
    }
    if daemon.minor < SERVES_FROM.minor {
        return Err(format!(
            "the daemon speaks API {daemon}, and this client speaks {API}, which changed \
             incompatibly after it; update Muster"
        ));
    }
    Ok(())
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
    fn a_client_of_this_major_from_the_oldest_served_on_is_served_newer_ones_included() {
        assert!(serves(&API).is_ok());
        assert!(serves(&SERVES_FROM).is_ok());
        assert!(serves(&Version { major: API.major, minor: API.minor + 1 }).is_ok());
    }

    #[test]
    fn a_client_reads_a_daemon_of_its_major_from_the_oldest_it_serves_on_and_no_other() {
        assert!(understands(&API).is_ok());
        assert!(understands(&Version { major: API.major, minor: API.minor + 1 }).is_ok());
        let other = understands(&Version { major: API.major + 1, minor: 0 }).unwrap_err();
        assert!(other.contains("major"), "{other}");
        if let Some(minor) = SERVES_FROM.minor.checked_sub(1) {
            let older = Version { major: API.major, minor };
            assert!(understands(&older).unwrap_err().contains("update Muster"), "{older}");
        }
    }

    #[test]
    fn a_client_of_another_major_or_from_before_the_oldest_served_is_refused_saying_why() {
        let other = serves(&Version { major: API.major + 1, minor: 0 }).unwrap_err();
        assert!(other.contains("major"), "{other}");
        if let Some(minor) = SERVES_FROM.minor.checked_sub(1) {
            let older = Version { major: API.major, minor };
            assert!(serves(&older).unwrap_err().contains("update the client"), "{older}");
        }
    }
}
