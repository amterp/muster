//! Binds libghostty-vt's C API, at the pinned commit.
//!
//! Generated here rather than checked in, because `deps/` is reproduced from
//! `deps/ghostty.pin` and is not in the repo: a checked-in `bindings.rs` would be derived
//! from a file nobody could diff it against. Generating from the header that `./dev -d`
//! produced means the bindings cannot drift from the pin by construction.
//!
//! Hand-written externs were the alternative and were rejected on the structs:
//! `GhosttyCell` and `GhosttyGridRef` cross by value, and a wrong layout guess is a silent
//! wrong answer rather than a link error.

use std::path::PathBuf;

fn main() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect(
        "the muster checkout should be two levels above this crate; if this moved, so did \
         the path to deps/",
    );
    let include = repo.join("deps/ghostty/zig-out/include");
    let lib = repo.join("deps/ghostty/zig-out/lib");
    let header = include.join("ghostty/vt.h");

    assert!(
        header.exists(),
        "no libghostty-vt header at {}. It is built from deps/ghostty.pin rather than \
         checked in, so a fresh checkout has none until `./dev` has run - any tier that \
         compiles takes it on the way past, so seeing this means cargo was run directly.",
        header.display()
    );

    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=dylib=ghostty-vt");
    // Where a binary looks for the dylib at *runtime* is set in .cargo/config.toml, not
    // here: a link argument emitted by a build script reaches only its own crate, and every
    // downstream binary would link fine and fail at startup.

    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", include.display()))
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .generate()
        .expect(
            "bindgen could not read libghostty-vt's header. It needs libclang, which comes \
             with the Xcode command line tools: xcode-select --install",
        );

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    bindings.write_to_file(out.join("bindings.rs")).expect("OUT_DIR should be writable");
}
