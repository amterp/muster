//! Muster's vocabulary, on its own, so that both ends of it can be built separately.
//!
//! The schema was generated inside `muster-seam` until the CLI existed, which was fine while
//! the only caller was in the same process. It is not fine now: the seam links `muster-vt`,
//! which links libghostty-vt as a dylib, so a client that reached the schema through the seam
//! would be a binary that will not start on a machine without that library - and the CLI is
//! the one part of Muster somebody copies onto a server.
//!
//! So this crate holds the generated types and nothing else that could drag a dependency in.
//! It is what the app and the CLI agree on, which is exactly the set of things two separately
//! built programs must not disagree about: the messages, and the framing that carries them.
//!
//! Deliberately not here: the names of the variables a pane is handed. Those are set by the
//! herdr adapter, which must not depend on the wire format - a schema in the adapter's
//! dependency graph is an invitation to translate protobuf where translation does not belong.
//! `crates/muster-cli/src/environment.rs` carries the CLI's own spelling and a test that the
//! two still agree.

pub mod frame;

include!(concat!(env!("OUT_DIR"), "/muster.rs"));

impl Response {
    /// Nothing to report, which is what most requests answer.
    ///
    /// Public because the endpoint's callers are in another crate now. It stays a constructor
    /// rather than something each caller spells out, so "success" has one spelling.
    pub fn ok() -> Response {
        Response { payload: Some(response::Payload::Ok(Ok {})) }
    }

    /// A refusal, written for whoever finds it in a log rather than for a caller that
    /// might branch on it.
    pub fn failure(reason: impl Into<String>) -> Response {
        Response { payload: Some(response::Payload::Failure(Failure { reason: reason.into() })) }
    }
}
