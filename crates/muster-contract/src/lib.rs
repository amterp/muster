//! The contract tier: the real app, launched against a real muster-daemon, judged by its run log.
//!
//! Everything is in `tests/launch.rs`. This file exists because cargo will not build a package
//! with no library or binary in it, and the checks need a package of their own: every one of
//! them is ignored in the default gate, since it needs a logged-in GUI session, and
//! `./dev --contract` runs exactly this package's ignored tests.
