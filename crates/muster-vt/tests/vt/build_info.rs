//! The engine says which engine it is.
//!
//! Guarded because the failure is silent: `engine_version` answering `None` puts
//! "unknown" in the run log rather than stopping anything, so a pin bump that renamed the
//! API would cost every future bug report its most useful line and nothing would say so.

#[test]
fn the_vt_engine_names_a_version() {
    let version = muster_vt::engine_version().expect("libghostty-vt reports a version");

    // Not the exact string - it moves with the pin, and pinning it here would make this a
    // test of deps/ghostty.pin rather than of the call. A version with a digit in it is
    // the whole claim.
    assert!(version.chars().any(|c| c.is_ascii_digit()), "{version:?}");
}
