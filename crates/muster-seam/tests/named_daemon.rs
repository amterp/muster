//! A daemon the config named, and what happens when it does not answer.
//!
//! The failure this is about is not a refusal - it is a window that quietly opened onto
//! somebody else's daemon and reported nothing. `follow_implicitly_if_nothing_else` says in
//! its own first line that it is for "when no config file named any", and it asked a different
//! question: whether anything is currently being followed. A config that named a daemon and
//! failed to reach it answers that question the same way an empty config does, so the window
//! attached Muster's own daemon under the configured daemon's id and carried on.
//!
//! Found by measuring the suite rather than by reading it. Under load the seam's daemon-backed
//! tests were reaching `~/.config/herdr/sessions/muster/herdr.sock` - a developer's own live
//! session, in a suite whose isolation `docs/testing.md` states as "nothing here can touch a
//! session someone is working in" (kan a_2L19sAmLZ). One 500ms snapshot that did not answer in
//! time was the whole of what it took.
//!
//! Its own binary because the seam holds one session per process, and this needs a launch that
//! has never reached a daemon at all.

use std::path::PathBuf;

use muster::proto::{OpenWindow, Request, Response, Startup, request, response};
use prost::Message;

#[test]
fn a_named_daemon_that_does_not_answer_is_not_replaced_by_another() {
    // Before anything reads it, and before the seam has a session: `own_socket_path` resolves
    // through this, so a scratch value is what stops the assertion below from being made
    // against a daemon somebody is working in. Sound here and nowhere near a `#[test]` that
    // shares its process - this binary holds one test, which is the reason it is its own file.
    let scratch = scratch_config_home();
    // SAFETY: no other thread exists yet. Nothing in this binary has been started, and the
    // seam reads the environment on the first request rather than at load.
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", &scratch);
        std::env::set_var("HOME", &scratch);
    }

    let _turn = muster::testing::fresh_session();

    // A socket path with nothing behind it, which is what a daemon that has stopped answering
    // looks like from here. Named in the config, so the window has been told which daemon it
    // is for - that is the whole difference between this and a launch with no config, and it
    // is the difference the code under test was not making.
    let silent = scratch.join("silent.sock");
    let config = scratch.join("muster.toml");
    std::fs::write(
        &config,
        format!("[[daemon]]\nid = \"local\"\nsocket = {:?}\n", silent.to_string_lossy()),
    )
    .expect("the scratch directory should be writable");

    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));

    let refusal = reason(&answer(request::Payload::OpenWindow(OpenWindow {})));

    // The socket the config named, and no other. A window that cannot reach the daemon it was
    // pointed at has to say so about *that* daemon: substituting another one renders panes
    // from a session nobody asked about, under the name of the session they did, and the
    // person reading the roster has no way to tell.
    assert!(
        refusal.contains(&silent.to_string_lossy().to_string()),
        "opening onto a daemon that does not answer should name the socket the config gave \
         it, and named none of it.\n  Impact: the window attached some other daemon under the \
         configured daemon's id, so what it shows belongs to a session nobody asked \
         for.\n  What it said instead: {refusal}"
    );
}

/// A config directory this test owns, so nothing here can resolve to a real one.
fn scratch_config_home() -> PathBuf {
    let path = PathBuf::from(format!("/tmp/muster-test/named-daemon-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("the harness root should be writable");
    path
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Made(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}

/// Why the core refused, or a panic naming what it did instead.
fn reason(response: &Response) -> String {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => failure.reason.clone(),
        other => panic!(
            "opening onto a daemon with nothing behind its socket should be refused, and the \
             core answered {other:?}"
        ),
    }
}
