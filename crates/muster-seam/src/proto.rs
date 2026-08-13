//! The generated types, and the small vocabulary for building answers.
//!
//! Kept in one module so that the rest of the crate can be read without knowing which
//! names came from prost and which we wrote.

include!(concat!(env!("OUT_DIR"), "/muster.rs"));

impl Response {
    pub(crate) fn ok() -> Response {
        Response { payload: Some(response::Payload::Ok(Ok {})) }
    }

    /// A refusal, written for whoever finds it in a log rather than for a caller that
    /// might branch on it.
    pub(crate) fn failure(reason: impl Into<String>) -> Response {
        Response { payload: Some(response::Payload::Failure(Failure { reason: reason.into() })) }
    }
}
