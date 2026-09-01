//! Which herdr a bridge actually runs.
//!
//! One question with two answers is what shipped. The app resolves the daemon in
//! `HerdrLocation.swift` and got it right when the daemon moved into the app's helper bundle;
//! the bridge resolved it a second time, looking for a file called `herdr` beside its own
//! executable, and nothing made the two move together. Every pane of the 0.3.0 cask then
//! rendered nothing (kan a_2Hnh3g0Y5).
//!
//! The fallback could not rescue it either, which is why "it will find one on PATH" is not an
//! answer: libghostty spawns the bridge, so the bridge inherits the *app's* environment, and an
//! app opened by Launch Services is handed launchd's `PATH=/usr/bin:/bin:/usr/sbin:/sbin` -
//! every directory on it SIP-protected, so there is nowhere to put a herdr even deliberately.
//!
//! So these drive the real binary and ask which program it started. No daemon: the fake herdr
//! records that it ran and exits, the bridge's frame pump reaches end of stream, and the
//! process ends on its own.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[test]
fn the_daemon_it_is_told_about_beats_the_one_on_path() {
    let scratch = scratch("told");
    let told = fake_herdr(&scratch, "told");
    let on_path = scratch.join("path-entry");
    fake_herdr(&on_path, "path");

    run_bridge(&["--herdr-binary".to_string(), told.display().to_string()], &on_path);

    assert!(
        marker(&scratch, "told").exists(),
        "the bridge was told to run {} and ran something else. Every pane of a shipped bundle \
         renders nothing when this is wrong, because the daemon there is not where a bridge \
         would look for one.",
        told.display()
    );
    assert!(
        !marker(&on_path, "path").exists(),
        "the bridge ran the herdr on PATH despite being told which one to run. A PATH lookup \
         finds whatever version somebody installed, so the pane would be rendered by a daemon \
         nobody pinned - and the frames are the one thing this project has a recorded corpus \
         for."
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn a_daemon_that_is_not_there_is_named_in_the_failure() {
    let scratch = scratch("missing");
    let absent = scratch.join("no-such-herdr");

    let output =
        run_bridge(&["--herdr-binary".to_string(), absent.display().to_string()], &scratch);

    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains(&absent.display().to_string()),
        "a bridge that could not start its daemon said which pane and not which file:\n{said}\n\
         That is the message the released cask printed, and working out that the file was \
         missing from the bundle rather than broken took a machine to reproduce it on."
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Runs a real bridge with a PATH holding only this directory, and waits for it to finish.
///
/// Pipes rather than a PTY, which the bridge already handles: it reads the surface's geometry
/// from stdout and falls back to 80x24 when there is no terminal there. What is under test is
/// which program it spawned, and that is decided before any of that matters.
fn run_bridge(arguments: &[String], path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_muster-bridge"))
        .arg("w1:p1")
        .args(arguments)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("cargo builds muster-bridge before this test runs")
}

/// A program that is not herdr, records that it was the one that ran, and ends.
///
/// Ending is what lets the bridge exit rather than being killed: its frame pump stops at end
/// of stream, which is the same path a real daemon that hung up takes.
fn fake_herdr(directory: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(directory).expect("the scratch root should be writable");
    let script = directory.join("herdr");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf '' > {}\n", marker(directory, name).display()),
    )
    .expect("the scratch root should be writable");
    make_executable(&script);
    script
}

fn marker(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}.ran"))
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut mode = std::fs::metadata(path).expect("just written").permissions();
    mode.set_mode(0o755);
    std::fs::set_permissions(path, mode).expect("the scratch root should be writable");
}

fn scratch(name: &str) -> PathBuf {
    let path =
        PathBuf::from(format!("/tmp/muster-test/bridge-daemon-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("the harness root should be writable");
    path
}
