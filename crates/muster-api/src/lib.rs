//! Muster's API for other programs (MIP-10): the generated messages of `proto/muster_api.proto`,
//! whose header is the API's documentation, its version rule, and the client's half of opening a
//! connection. A program written in Rust can depend on this crate; one in any other language
//! generates its own bindings from the schema.

pub mod connection;
pub mod version;

include!(concat!(env!("OUT_DIR"), "/muster.api.rs"));
