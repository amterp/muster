//! Generates the API's types from `proto/muster_api.proto`, with protox rather than protoc as
//! muster-daemon-proto does: no binary on PATH, and nothing generated is committed.

use std::path::PathBuf;

fn main() {
    let proto = PathBuf::from("../../proto/muster_api.proto");
    let root = proto.parent().expect("the schema has a directory");
    println!("cargo:rerun-if-changed={}", proto.display());
    println!("cargo:rerun-if-changed=build.rs");
    let descriptors = protox::compile([&proto], [root]).expect("the schema compiles");
    let mut config = prost_build::Config::new();
    config.skip_protoc_run();
    config.compile_fds(descriptors).expect("the schema generates");
}
