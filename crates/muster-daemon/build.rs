//! The Ghostty version a pane says it runs in (`TERM_PROGRAM_VERSION`), read from the pinned
//! checkout: the version its `build.zig.zon` declares, and the pinned commit. And, on a Mac,
//! where the daemon looks for libghostty-vt besides the checkout.
//!
//! Ghostty's own build says different things for one commit depending on how it was built - a
//! branch name, `HEAD`, or no commit at all - so none of those is the answer for a daemon built
//! from the pin. The declared version and the commit are what stay true.

use std::path::PathBuf;

fn main() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let zon = repo.join("deps/ghostty/build.zig.zon");
    let pin = repo.join("deps/ghostty.pin");
    println!("cargo:rerun-if-changed={}", zon.display());
    println!("cargo:rerun-if-changed={}", pin.display());

    let zon = std::fs::read_to_string(&zon).unwrap_or_else(|error| {
        panic!(
            "could not read {}: {error}. It is fetched from deps/ghostty.pin rather than checked \
             in, so a fresh checkout has none until ./dev has run.",
            zon.display()
        )
    });
    let version = zon
        .lines()
        .find_map(|line| line.trim().strip_prefix(".version = \""))
        .and_then(|rest| rest.split_once('"'))
        .map(|(version, _)| version)
        .expect("build.zig.zon declares .version = \"...\"");
    let pin = std::fs::read_to_string(&pin).expect("deps/ghostty.pin is checked in");
    let commit = pin.trim().get(..8).expect("deps/ghostty.pin holds a full commit hash");
    println!("cargo:rustc-env=MUSTER_GHOSTTY_VERSION={version}+{commit}");

    // Beside itself, which is how a remote Mac has it: the app copies this daemon there with
    // the libghostty-vt it links (muster-daemon-client's `install`), and the checkout's own
    // rpaths name nothing on that machine.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path");
    }
}
