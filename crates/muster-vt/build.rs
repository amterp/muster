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

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect(
        "the muster checkout should be two levels above this crate; if this moved, so did \
         the path to deps/",
    );
    let include = repo.join("deps/ghostty/zig-out/include");
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let lib = repo.join("deps/ghostty").join(library_dir(&target));
    let header = include.join("ghostty/vt.h");

    assert!(
        header.exists(),
        "no libghostty-vt header at {}. It is built from deps/ghostty.pin rather than \
         checked in, so a fresh checkout has none until `./dev` has run - any tier that \
         compiles takes it on the way past, so seeing this means cargo was run directly.",
        header.display()
    );

    // The directory rather than vt.h: vt.h only includes the headers that declare anything,
    // and a change to one of those alone - a re-pin, a patch in deps/ghostty-patches/ - left
    // the bindings stale. Cargo scans a directory for any file that changed.
    println!("cargo:rerun-if-changed={}", include.join("ghostty").display());
    // Statically, everywhere. A daemon copied to another machine is then one file, and nothing
    // in the bundle loads a libghostty-vt from beside itself. The app's process holds two copies
    // of libghostty - GhosttyKit's, linked into the app, and this one inside libmuster - and that
    // is safe only because libmuster is a cdylib of its own that exports nothing but
    // `include/muster.h`'s symbols (MIP-1): the two archives define hundreds of the same strong
    // symbols, and cannot be linked into one image
    // (docs/observations/libghostty-0fa3d577.md, section 8).
    //
    // From a directory holding only the archive, because zig's prefix holds the dylib beside it:
    // rustc bundles the archive into this crate for every crate that depends on it, but links
    // this crate's own test executable with `-lghostty-vt`, and the macOS linker takes a dylib
    // over an archive when it finds both. A rebuilt archive is noticed only because cargo is
    // told to watch it: a patch that changes only Zig code leaves every header alone.
    let archive = lib.join("libghostty-vt.a");
    println!("cargo:rerun-if-changed={}", archive.display());
    let archive_only =
        PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("libghostty-vt");
    std::fs::create_dir_all(&archive_only).expect("OUT_DIR should be writable");
    let link = archive_only.join("libghostty-vt.a");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&archive, &link).expect("OUT_DIR should be writable");
    println!("cargo:rustc-link-search=native={}", archive_only.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");

    let mut bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", include.display()));
    // bindgen hands clang the target cargo builds for, and clang defers to musl's own
    // stddef.h and stdint.h there, which a Mac does not have. Zig carries them, and they are
    // the headers the library itself was compiled against.
    if let Some(zig_target) = linux_target(&target) {
        println!("cargo:rerun-if-changed={}", repo.join("deps/zig.pin").display());
        let libc = zig_lib_dir(&repo).join("libc/include");
        for dir in [zig_target, "generic-musl", "any-linux-any"] {
            bindings = bindings.clang_arg(format!("-isystem{}", libc.join(dir).display()));
        }
    }
    let bindings = bindings
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

    let modes = include.join("ghostty/vt/modes.h");
    std::fs::write(out.join("modes.rs"), mode_table(&modes)).expect("OUT_DIR should be writable");
}

/// Where `./dev` put the libghostty-vt built for `target`, under `deps/ghostty`: the host's
/// in zig's default prefix, and each Linux daemon's in a prefix of its own, because a build
/// for another target into the default prefix would replace the host's archive.
fn library_dir(target: &str) -> String {
    match linux_target(target) {
        Some(zig_target) => format!("zig-out/{zig_target}/lib"),
        None => "zig-out/lib".to_string(),
    }
}

/// Zig's name for a Linux target the daemon is built for (MIP-3, section 12).
fn linux_target(target: &str) -> Option<&'static str> {
    match target {
        "x86_64-unknown-linux-musl" => Some("x86_64-linux-musl"),
        "aarch64-unknown-linux-musl" => Some("aarch64-linux-musl"),
        _ => None,
    }
}

/// The directory the pinned zig keeps its libc headers under, as `zig env` reports it.
fn zig_lib_dir(repo: &std::path::Path) -> PathBuf {
    let zig = repo.join("tools/zig");
    let output = std::process::Command::new(&zig).arg("env").output().unwrap_or_else(|error| {
        panic!(
            "could not run {}, which runs the zig deps/zig.pin names and is needed to build for \
             a Linux target, as it is for libghostty itself: {error}",
            zig.display()
        )
    });
    assert!(
        output.status.success(),
        "{} env failed, so the Linux headers bindgen needs cannot be found:\n{}",
        zig.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let env = String::from_utf8_lossy(&output.stdout);
    let dir = env
        .lines()
        .find_map(|line| line.trim().strip_prefix(".lib_dir = \""))
        .and_then(|rest| rest.strip_suffix("\","))
        .unwrap_or_else(|| panic!("`zig env` names no .lib_dir; it printed:\n{env}"));
    PathBuf::from(dir)
}

/// Every mode `modes.h` names, as a Rust table.
///
/// The header spells modes as function-like macros over a `static inline` constructor, which
/// bindgen cannot evaluate, so without this the list would be typed out by hand - and a pin
/// that added a mode would leave the replay silently not carrying it. Parsing the header
/// makes the table the pin's by construction, the same argument as the bindings above.
fn mode_table(header: &std::path::Path) -> String {
    let text = std::fs::read_to_string(header).expect("modes.h sits beside vt.h");
    let mut table = String::from("pub(crate) const MODES: &[(&str, u16, bool)] = &[\n");
    let mut count = 0;
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("#define GHOSTTY_MODE_") else { continue };
        let Some((name, definition)) = rest.split_once(char::is_whitespace) else { continue };
        let Some(arguments) = definition
            .trim()
            .strip_prefix("(ghostty_mode_new(")
            .and_then(|rest| rest.split_once("))"))
            .map(|(arguments, _)| arguments)
        else {
            continue;
        };
        let (value, ansi) =
            arguments.split_once(',').expect("ghostty_mode_new takes two arguments");
        let value: u16 = value.trim().parse().expect("a mode's value is a number");
        let ansi: bool = ansi.trim().parse().expect("a mode's ANSI flag is true or false");
        writeln!(table, "    ({name:?}, {value}, {ansi}),")
            .expect("writing to a String cannot fail");
        count += 1;
    }
    assert!(
        count > 30,
        "found only {count} modes in {}. The header's spelling of them changed, and a short \
         table would make the replay drop modes without saying so - fix the parser here.",
        header.display()
    );
    table.push_str("];\n");
    table
}
