//! Generates the seam's message types, and names the library the way a loader asks for it.
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
    config.compile_fds(descriptors).expect("the schema generates");

    // cargo's default install name is the absolute path of the build output, which works
    // on the machine that produced it and nowhere else - and it points into
    // `target/debug/deps` rather than at the copy anything else would find. `@rpath`
    // instead, so the binary that links this says "find libmuster.dylib on my rpath" and a
    // checkout and a shipped bundle can each answer that their own way.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libmuster.dylib");
    }
}
