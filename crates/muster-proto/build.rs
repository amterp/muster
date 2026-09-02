//! Generates Muster's message types from the committed schema.
//!
//! protox rather than protoc: it is a protobuf compiler written in Rust, so `cargo build`
//! needs no binary on PATH and rust-analyzer keeps working in a bare checkout. The Swift
//! side has no equivalent and uses swift-protobuf's own protoc, which `./dev` builds from
//! source - but both sides compile the same committed `.proto`, so neither can drift from
//! it or from each other.
//!
//! Nothing generated is committed. Cargo's own freshness tracking is the staleness check
//! on this side: touch the schema and the next build regenerates.

use std::path::PathBuf;

fn main() {
    let proto = PathBuf::from("../../proto/muster.proto");
    let root = proto.parent().expect("the schema has a directory");
    println!("cargo:rerun-if-changed={}", proto.display());
    println!("cargo:rerun-if-changed=build.rs");

    let descriptors = protox::compile([&proto], [root]).expect("the schema compiles");
    let mut config = prost_build::Config::new();
    // protox already parsed it; without this prost goes looking for a protoc binary that
    // this build deliberately does not require.
    config.skip_protoc_run();
    // Every response is returned by value, including the one answering a keystroke, so the
    // size of the largest variant is paid on every request rather than on the one that needs
    // it. Appearance is that variant several times over - eight colours, a family name and a
    // palette - and is read exactly once, at launch. Boxing puts the cost where it belongs.
    // The Swift side needs no equivalent: swift-protobuf's messages are already references.
    config.boxed("Response.payload.appearance");
    // The same argument on the other side of the seam, and it bites harder there. Every event
    // is built by value, most of them are one pane's state change, and an agent transition is
    // supposed to cost that transition (`architecture.md`, fast is a feature). Appearance is
    // the largest variant by several times over and is sent when somebody saves a file.
    config.boxed("Event.payload.appearance_changed");
    config.compile_fds(descriptors).expect("the schema generates");
}
