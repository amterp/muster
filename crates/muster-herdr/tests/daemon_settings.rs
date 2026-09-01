//! Whether Muster's own config decides what a Muster pane runs.
//!
//! The corpus pins the translation - Muster's two settings in, herdr's keys out - and it can
//! pin nothing else, because herdr exposes no way to read its config back. So the claim that
//! the daemon *reads* Muster's file is answered the only way it can be: a real daemon, a real
//! pane, and a shell that says which config won by existing.
//!
//! Two things are proved together because they are two halves of one arrangement. The daemon
//! reads Muster's file rather than the user's, and a pane made through the real request path
//! is handed the user's file back - so `herdr` inside a Muster pane still reads what it always
//! did. Get the first without the second and every pane's own herdr silently reads Muster's.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use herdr_harness::{binary, until_some};
use muster_core::config;
use muster_core::intent::{BackendChannel, BackendIntent};
use muster_core::names::{Mint, Names};
use muster_herdr::{HerdrBackend, HerdrClient, PaneEnvironment, daemon, own_socket_path};
use serde_json::json;

/// What the shell would report the platform's locale as, so this passes on a Mac set to
/// anything.
const PLATFORM_LOCALE: &str = "en_AU.UTF-8";

#[test]
fn a_pane_runs_the_shell_musters_own_config_names() {
    let root = PathBuf::from(format!("/tmp/mds{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for directory in ["config/herdr", "home", "state"] {
        std::fs::create_dir_all(root.join(directory)).expect("could not build the test root");
    }

    // Two shells that differ only in which config names them, so the assertion at the end is
    // about whose file won and nothing else. Both end in `/bin/sh` because a pane whose
    // program exits takes the pane, then the workspace, then the headless server with it.
    let theirs = shell(&root, "theirs");
    let ours = shell(&root, "ours");

    // The user's own herdr config, which the daemon must not read. Update checks off here as
    // well as in Muster's file, so that a test on a machine with no network cannot be waiting
    // on one either way.
    let user_config = root.join("config/herdr/config.toml");
    std::fs::write(
        &user_config,
        format!(
            "[terminal]\ndefault_shell = {theirs:?}\nshell_mode = \"non_login\"\n\
             new_cwd = \"current\"\n\n[update]\nversion_check = false\nmanifest_check = false\n"
        ),
    )
    .expect("could not write the user's own herdr config");

    // Muster's, derived the way a launch derives it: from a config file a person wrote.
    let parsed = config::parse(&format!(
        "scrollback_bytes = 4096\n\n[shell]\ncommand = {ours:?}\nmode = \"non_login\"\n"
    ))
    .expect("this is a config file Muster accepts");
    let derived = root.join("state/herdr.toml").display().to_string();
    muster_herdr::write_configuration(&derived, &parsed.panes).expect("the test root is writable");

    // Appended, and the one line of this file the test writes itself. Muster deliberately no
    // longer turns manifest checks off (a_2HxSqYtuA) - a frozen manifest is how `working`
    // became unreachable - so a daemon started from the file above would fetch from herdr.dev,
    // and the gate reaches no network. What this test asserts is which config file decides a
    // pane's shell, and that is untouched by the line; what the file says about updates is
    // pinned by corpus/conformance/daemon-config.json, where nothing has to run.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&derived)
        .and_then(|mut file| file.write_all(b"manifest_check = false\n"))
        .expect("the derived config was just written here");

    let mut environment = BTreeMap::new();
    environment.insert("PATH".to_string(), std::env::var("PATH").unwrap_or_default());
    environment.insert("HOME".to_string(), root.join("home").display().to_string());
    environment.insert("XDG_CONFIG_HOME".to_string(), root.join("config").display().to_string());
    environment.insert("XDG_STATE_HOME".to_string(), root.join("state").display().to_string());

    let socket = own_socket_path(&environment).expect("the environment says where home is");
    assert!(
        socket.len() <= 104,
        "the test root yields a {}-byte socket path, over the sockaddr_un limit - herdr would \
         exit with InvalidInput and this would read as the wrong config winning.",
        socket.len()
    );

    // No command directory: this test is about which config file a pane's shell reads, and putting
    // a `muster` on that shell's PATH would be a second thing changing at the same time.
    daemon::start(&binary(), &socket, &environment, Some(PLATFORM_LOCALE), Some(&derived), None)
        .expect("the pinned daemon should start");
    let stopped = Stop(socket.clone());

    // Through the real request path rather than a hand-built call, because the environment a
    // pane is handed is built there - a test that called `workspace.create` itself would prove
    // the daemon's half and quietly skip Muster's.
    let backend = HerdrBackend::new(
        HerdrClient::new(&socket),
        PaneEnvironment::restoring(&environment),
        Names::alone("local", Mint::Backend),
    );
    backend
        .submit(&BackendIntent::CreateWorkspace {
            cwd: Some(root.display().to_string()),
            run: None,
            name: None,
        })
        .expect("a daemon that answered ping can make a workspace");

    let client = HerdrClient::new(&socket);
    let pane = until_some("the daemon to describe the pane it just made", || {
        let answer = client.request("pane.list", &json!({})).ok()?;
        let panes = answer.get("payload").unwrap_or(&answer).get("panes")?.as_array()?;
        Some(panes.first()?.get("pane_id")?.as_str()?.to_string())
    });

    // Written to a file rather than read off the screen, for the reason `daemon_isolation`
    // gives: a grid wraps and carries the shell's own echo, and this is a question about a
    // process rather than about rendering.
    let dump = root.join("environment.txt");
    client
        .request(
            "pane.send_text",
            &json!({ "pane_id": pane, "text": format!("env > {}\n", dump.display()) }),
        )
        .expect("a pane accepts text");
    let written = until_some("the shell in the pane to write its environment out", || {
        let text = std::fs::read_to_string(&dump).ok()?;
        text.contains("PATH=").then_some(text)
    });
    drop(stopped);

    let ran_ours = root.join("ours.ran").exists();
    let ran_theirs = root.join("theirs.ran").exists();
    assert!(
        ran_ours && !ran_theirs,
        "the pane ran {} rather than the shell Muster's own config named.\n  Impact: what a \
         pane runs is decided by whatever `default_shell` somebody set for their own terminal, \
         which is the whole of what this card removed - and the same file decides whether the \
         pinned daemon goes looking for its own updates.\n  Check that daemon::start still \
         supplies HERDR_CONFIG_PATH, and that the derived file at {derived} says what the \
         corpus says it should.",
        if ran_theirs { "the user's own shell" } else { "neither shell" }
    );

    assert!(
        written.lines().any(|line| line == format!("HERDR_CONFIG_PATH={}", user_config.display())),
        "a pane was not handed the user's own herdr config path back.\n  Impact: `herdr` run \
         inside a Muster pane reads the config file Muster wrote for its daemon rather than the \
         person's own - which looks like nothing at all until somebody wonders why their own \
         settings stopped applying.\n  Check PaneEnvironment::restoring and that every \
         pane-creating arm of `request` carries what it returns.\n  The pane held: {:?}",
        written.lines().filter(|line| line.starts_with("HERDR_")).collect::<Vec<&str>>()
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A shell that records having been run, and then behaves like one.
fn shell(root: &Path, name: &str) -> String {
    let path = root.join(format!("{name}-shell"));
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\ntouch {}\nexec /bin/sh \"$@\"\n",
            root.join(format!("{name}.ran")).display()
        ),
    )
    .expect("could not write the test's shell");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("could not make the test's shell runnable");
    }
    path.display().to_string()
}

/// Stops the daemon when the test leaves, however it leaves.
struct Stop(String);

impl Drop for Stop {
    fn drop(&mut self) {
        let _ = HerdrClient::new(&self.0).request("server.stop", &json!({}));
    }
}
