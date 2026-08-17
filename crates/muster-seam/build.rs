//! Names the library the way a loader asks for it.
//!
//! The message types come from `muster-proto` rather than from here, so that the CLI can link
//! the schema without linking this crate's dylib dependencies. All that is left is the one
//! thing only a cdylib has to answer.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // cargo's default install name is the absolute path of the build output, which works
    // on the machine that produced it and nowhere else - and it points into
    // `target/debug/deps` rather than at the copy anything else would find. `@rpath`
    // instead, so the binary that links this says "find libmuster.dylib on my rpath" and a
    // checkout and a shipped bundle can each answer that their own way.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libmuster.dylib");
    }
}
