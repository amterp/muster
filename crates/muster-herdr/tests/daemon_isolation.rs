//! Whether the shell that launched Muster reaches the agents Muster runs.
//!
//! The unit answer is `carried`, and it has a corpus. This is the other half: a real daemon,
//! started the way Muster starts one, spawning a real shell in a real pane - because the
//! claim is about what a *pane* ends up holding, and every step between the allowlist and
//! there is a step the corpus cannot see. herdr decides what a pane's process inherits, and
//! a herdr that one day passes an environment of its own would make the allowlist true and
//! the claim false.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use herdr_harness::binary;
use muster_herdr::{HerdrClient, daemon, own_socket_path};
use serde_json::json;

/// A marker no shell would ever set for itself, spelled the way the real one was.
///
/// The variable that started this: a Claude Code launched inside a Muster pane read its
/// launching session's marker, decided it was a child session, and silently stopped saving
/// its own transcript.
const HARNESS_MARKER: &str = "CLAUDE_CODE_CHILD_SESSION";

/// A credential, so the test fails loudly on the case that matters most.
const HARNESS_TOKEN: &str = "CLAUDE_CODE_MESSAGING_TOKEN";

#[test]
fn a_pane_does_not_inherit_the_launching_sessions_private_state() {
    let root = PathBuf::from(format!("/tmp/mdi{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for directory in ["config/herdr", "home", "state"] {
        std::fs::create_dir_all(root.join(directory)).expect("could not build the test root");
    }
    std::fs::write(
        root.join("config/herdr/config.toml"),
        "[terminal]\ndefault_shell = \"/bin/sh\"\nshell_mode = \"non_login\"\n\
         new_cwd = \"current\"\n\n[update]\nversion_check = false\nmanifest_check = false\n",
    )
    .expect("could not write the daemon's config");

    // What a launch from inside a coding-agent session actually looks like: the things a
    // daemon needs, and beside them one session's private state.
    let mut environment = BTreeMap::new();
    environment.insert("PATH".to_string(), std::env::var("PATH").unwrap_or_default());
    environment.insert("HOME".to_string(), root.join("home").display().to_string());
    environment.insert("XDG_CONFIG_HOME".to_string(), root.join("config").display().to_string());
    environment.insert("XDG_STATE_HOME".to_string(), root.join("state").display().to_string());
    environment.insert(HARNESS_MARKER.to_string(), "1".to_string());
    environment.insert(HARNESS_TOKEN.to_string(), "this-must-not-reach-an-agent".to_string());

    let socket = own_socket_path(&environment).expect("the environment says where home is");
    assert!(
        socket.len() <= 104,
        "the test root yields a {}-byte socket path, over the sockaddr_un limit - herdr would \
         exit with InvalidInput and this would read as a scrubbing failure.",
        socket.len()
    );

    daemon::start(&binary(), &socket, &environment).expect("the pinned daemon should start");
    let client = HerdrClient::new(&socket);
    let stopped = Stop(socket.clone());

    client
        .request("workspace.create", &json!({ "cwd": root.display().to_string(), "focus": true }))
        .expect("a daemon that answered ping can make a workspace");

    let pane = until("the daemon to describe the pane it just made", || {
        let answer = client.request("pane.list", &json!({})).ok()?;
        let panes = answer.get("payload").unwrap_or(&answer).get("panes")?.as_array()?;
        Some(panes.first()?.get("pane_id")?.as_str()?.to_string())
    });

    // Written to a file rather than read off the screen. A pane's grid wraps at its width and
    // carries the shell's own prompt and echo, so a test reading it would be asserting on
    // terminal rendering to answer a question about process environment.
    let dump = root.join("environment.txt");
    client
        .request(
            "pane.send_text",
            &json!({ "pane_id": pane, "text": format!("env > {}\n", dump.display()) }),
        )
        .expect("a pane accepts text");

    let written = until("the shell in the pane to write its environment out", || {
        let text = std::fs::read_to_string(&dump).ok()?;
        // The shell may be mid-write; wait for the variable that proves it got that far.
        text.contains("PATH=").then_some(text)
    });
    drop(stopped);

    let names: Vec<&str> =
        written.lines().filter_map(|line| line.split_once('=')).map(|(name, _)| name).collect();

    assert!(
        !names.contains(&HARNESS_MARKER),
        "a pane's shell inherited {HARNESS_MARKER} from whatever launched Muster.\n  Impact: \
         every agent started in a Muster pane is told it is a child session of somebody \
         else's, and silently stops saving its transcript. The daemon outlives the app, so \
         this persists after the Muster that carried it in has quit.\n  Check \
         daemon::carried, and whether daemon::start still builds the environment rather than \
         inheriting it.\n  The pane held: {names:?}"
    );
    assert!(
        !written.contains("this-must-not-reach-an-agent"),
        "a pane's shell was handed {HARNESS_TOKEN}, which is one session's credential for \
         talking to another.\n  Impact: any program in any Muster pane can use it, on any \
         machine this daemon outlives.\n  Check daemon::carried."
    );
    assert!(
        names.contains(&"PATH"),
        "a pane's shell has no PATH, so nothing in it can run a command.\n  Impact: every \
         pane is a shell that cannot find `ls`. Scrubbing went too far.\n  The pane held: \
         {names:?}"
    );
    assert!(
        names.contains(&"HOME"),
        "a pane's shell has no HOME.\n  Impact: anything reading a config or a history file \
         in a pane looks in the wrong place. Scrubbing went too far.\n  The pane held: \
         {names:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Stops the daemon when the test leaves, however it leaves.
///
/// A daemon Muster started is deliberately in its own process group and outlives whoever
/// started it, which is right in an app and wrong in a test: a panicking assertion would
/// otherwise leak a `herdr server` holding this root forever.
struct Stop(String);

impl Drop for Stop {
    fn drop(&mut self) {
        let _ = HerdrClient::new(&self.0).request("server.stop", &json!({}));
    }
}

fn until<T>(what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(found) = ready() {
            return found;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}
