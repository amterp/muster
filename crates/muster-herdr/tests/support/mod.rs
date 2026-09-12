//! What the devenv tests are handed by `./dev --ssh`: which machine to reach and where the
//! daemon for it already sits.

/// Where the container is, and how to reach it.
///
/// Read from the environment rather than hardcoded, because `./dev --ssh` owns the container and
/// knows the key it generated. A test that reconstructed the arguments would be a second copy of
/// that knowledge.
pub(crate) fn devenv() -> (String, Vec<String>) {
    let host = std::env::var("MUSTER_DEVENV_HOST").expect(
        "MUSTER_DEVENV_HOST is unset, so this test has no machine to talk to. Run it through \
         ./dev --ssh, which starts the container and sets it.",
    );
    let options = std::env::var("MUSTER_DEVENV_SSH_OPTIONS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    (host, options)
}

/// Where the pinned daemon for the container's platform already sits on this machine.
///
/// Handed over rather than downloaded here. `./dev --ssh` fetches it once and verifies it against
/// `deps/herdr.pin`, which is where every other acquisition in this repo happens - and it keeps
/// the tier offline, which `docs/testing.md` asks of every tier rather than only of the gate.
pub(crate) fn cache() -> String {
    std::env::var("MUSTER_DEVENV_CACHE").expect(
        "MUSTER_DEVENV_CACHE is unset, so this test has no daemon to install and would reach \
         the network for one. Run it through ./dev --ssh, which fetches the container's \
         platform asset against the pin and points this at it.",
    )
}
