//! Generates the daemon protocol's types, and fixes which install this build belongs to.
//!
//! protox rather than protoc, as in muster-proto: no binary on PATH, and nothing generated is
//! committed.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The messages a daemon writes into its persisted state as they are, so that a setting added
/// to `Settings` is kept across a restart without anyone remembering to. A new message a
/// setting uses goes here too; a new enum needs nothing, since enum fields are `i32`s, but a
/// oneof inside a persisted message is a Rust enum of its own and needs the same derive through
/// `enum_attribute`. Derived only with the `serde` feature, which only the daemon enables.
const PERSISTED: [&str; 5] = [
    ".muster.daemon.Settings",
    ".muster.daemon.Shell",
    ".muster.daemon.Palette",
    ".muster.daemon.Cursor",
    ".muster.daemon.Label",
];

fn main() {
    let proto = PathBuf::from("../../proto/muster_daemon.proto");
    let root = proto.parent().expect("the schema has a directory");
    println!("cargo:rerun-if-changed={}", proto.display());
    println!("cargo:rerun-if-changed=build.rs");

    let descriptors = protox::compile([&proto], [root]).expect("the schema compiles");
    let mut config = prost_build::Config::new();
    config.skip_protoc_run();
    if std::env::var_os("CARGO_FEATURE_SERDE").is_some() {
        for message in PERSISTED {
            // Defaults for what a file written before a field existed does not hold.
            config.message_attribute(
                message,
                "#[derive(serde::Serialize, serde::Deserialize)] #[serde(default)]",
            );
        }
    }
    // A handoff's pane dwarfs every other message it sends.
    config.boxed(".muster.daemon.Handoff.message.pane");
    // Mirrors the detector's own flags, which are herdr's.
    config.message_attribute(
        ".muster.daemon.Handoff.Detection",
        "#[expect(clippy::struct_excessive_bools, reason = \"the detector's flags, one each\")]",
    );
    config.message_attribute(
        ".muster.daemon.Pane",
        "#[expect(clippy::struct_excessive_bools, reason = \"independent facts, one each\")]",
    );
    config.compile_fds(descriptors).expect("the schema generates");

    println!("cargo:rerun-if-env-changed=MUSTER_INSTALL");
    println!("cargo:rustc-env=MUSTER_DAEMON_INSTALL={}", install());
}

/// The install this build's daemon and clients belong to (`install.rs` says why).
///
/// `MUSTER_INSTALL` names it for a build that ships. Otherwise it is this checkout: two working
/// trees built on one machine are two installs, because they are two versions of the code.
fn install() -> String {
    if let Ok(named) = std::env::var("MUSTER_INSTALL") {
        assert!(
            !named.is_empty()
                && named.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "MUSTER_INSTALL is {named:?}, and it names a socket file, so it has to be lowercase \
             letters, digits and hyphens."
        );
        return named;
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let workspace = Path::new(&manifest).join("../..");
    let workspace = workspace.canonicalize().unwrap_or(workspace);
    let digest = Sha256::digest(workspace.as_os_str().as_encoded_bytes());
    let mut name = String::from("dev-");
    for byte in &digest[..6] {
        write!(name, "{byte:02x}").expect("writing to a String cannot fail");
    }
    name
}
