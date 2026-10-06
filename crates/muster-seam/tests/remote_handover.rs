//! An older daemon on a devenv, handed over to this build's when a window attaches to it.
//!
//! What an update does on a remote machine: the app finds the daemon it installed there saying
//! an older version, installs this build's beside it, and asks the old one to hand its panes
//! over. Out of the default gate, and marked `#[ignore]` to keep it there: it needs the devenv
//! container (`docs/testing.md`), which `./dev --ssh` brings up.
//!
//! Its own binary because it points `MUSTER_HOME` at a scratch home before anything reads it,
//! so the window's state files are this test's rather than the developer's.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use muster::proto::{OpenWindow, Request, Response, Startup, request, response};
use muster_daemon_client::environment::for_far_daemon;
use muster_daemon_client::install::Carried;
use muster_daemon_client::launch::stop;
use muster_daemon_client::remote::{Installed, install, start_script};
use muster_daemon_proto::ConnectionKind;
use muster_daemon_proto::connection;
use muster_daemon_proto::input_event;
use muster_daemon_proto::launch::LAUNCH_PATIENCE;
use muster_harness::requests::{create, in_new_tab, make, read_text};
use muster_harness::{Control, DAEMON_DATA, Input, built_linux_daemons, until_some};
use muster_ssh::{Forward, Tunnel, remote_environment};
use prost::Message;

/// Where the container is and how to reach it, from `./dev --ssh`.
fn devenv() -> (String, Vec<String>) {
    let host = std::env::var("MUSTER_DEVENV_HOST").expect(
        "MUSTER_DEVENV_HOST is unset, so this test has no machine to talk to. Run it through \
         ./dev --ssh, which starts the container and sets it.",
    );
    let options = std::env::var("MUSTER_DEVENV_SSH_OPTIONS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    (host, options)
}

#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn an_older_daemon_on_a_devenv_is_handed_over_with_its_shells() {
    let home = scratch_home();
    let (host, options) = devenv();
    let far = remote_environment(&host, &options).unwrap();
    let installed = Installed::on(&far).expect("the container has a HOME");
    let temporary = std::env::temp_dir();
    let tunnel = Tunnel::open(
        Forward {
            host: host.clone(),
            options: options.clone(),
            control_path: temporary.join("muster-handover-test.ctl").to_string_lossy().into(),
            local_socket: temporary.join("muster-handover-test.sock").to_string_lossy().into(),
            remote_socket: installed.socket.to_string_lossy().into(),
            reverse: None,
        },
        Arc::new(|_| {}),
    )
    .expect("the tunnel opens");
    let local = PathBuf::from(tunnel.local_socket_path());
    let far_home = installed
        .directory
        .parent()
        .and_then(Path::parent)
        .expect("under ~/.muster")
        .to_string_lossy()
        .into_owned();
    // One left from an earlier run would be adopted as it is, saying this build's version.
    let _ = stop(&local, Duration::from_secs(10));
    let clean = || {
        let _ = tunnel.remote().shell(&format!("rm -rf {}", muster_ssh::quoted(&far_home)));
    };
    clean();
    let _cleaned = Finally(Box::new(|| {
        let _ = stop(&local, Duration::from_secs(10));
        clean();
    }));

    // The older daemon: this build's, installed where the app installs it and started the way
    // the app starts one, told to say a version before this one. The app starts a daemon with a
    // fixed environment, so the test starts this one itself.
    let carried = Carried {
        linux: Some(built_linux_daemons()),
        data: Some(DAEMON_DATA.into()),
        ..Carried::default()
    };
    install(&tunnel.remote(), &installed, &carried).expect("the daemon installs");
    let mut environment: BTreeMap<String, String> = for_far_daemon(&far);
    environment.insert("MUSTER_DAEMON_VERSION_SAID".to_string(), "0.0.1".to_string());
    tunnel
        .remote()
        .shell(&start_script(&installed, "older", &environment))
        .expect("the older daemon starts");
    let older = until_some("the older daemon to answer", || {
        connection::connect(&local, ConnectionKind::Control, "test").ok().map(|(_, hi)| hi)
    });
    assert_eq!(older.daemon_version, "0.0.1");
    let mut control = Control::connect(&local);
    make(&mut control, create("p1", in_new_tab("t1")));
    let mut input = Input::connect(&local);
    type_line(&mut input, "echo pid=$$.");
    let pid = until_some("the shell to say its pid", || {
        said(&read_text(&mut control, "p1", 0, 0).text, "pid")
    });
    drop((control, input));

    // Dropped after the turn and before the cleanup above: the window lets go of its daemon
    // and its own ssh master first, so it neither follows the daemon being stopped back into
    // life nor leaves a master behind holding this test's output open.
    let _closed = Finally(Box::new(|| drop(muster::testing::fresh_session())));
    let _turn = muster::testing::fresh_session();
    open_a_window(&home, &host, &options);

    let newer = handed_over(&local, older.instance, &home);
    assert_eq!(newer.daemon_version, env!("CARGO_PKG_VERSION"), "the new daemon answers");

    let mut control = Control::connect(&local);
    let mut input = Input::connect(&local);
    type_line(&mut input, "echo again=$$.");
    let again = until_some("the shell to say its pid again", || {
        said(&read_text(&mut control, "p1", 0, 0).text, "again")
    });
    assert_eq!(again, pid, "the pane's shell lived through the handoff");
}

/// Opens a window configured with the devenv's daemon and nothing else, which is the managed
/// arm: the one that installs this build there and asks an older daemon to hand over.
fn open_a_window(home: &Path, host: &str, options: &[String]) {
    let config = home.join("config.toml");
    let quoted: Vec<String> = options.iter().map(|option| format!("{option:?}")).collect();
    std::fs::write(
        &config,
        format!(
            "[[daemon]]\nid = \"devenv\"\nhost = {host:?}\nssh_options = [{}]\n",
            quoted.join(", ")
        ),
    )
    .expect("the config can be written");
    for payload in [
        request::Payload::Startup(Startup {
            config_path: config.to_string_lossy().into_owned(),
            daemon_data_path: DAEMON_DATA.to_string(),
            remote_daemons_path: built_linux_daemons().to_string_lossy().into_owned(),
            log_path: home.join("muster.jsonl").to_string_lossy().into_owned(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        assert_ok(&answer(payload));
    }
}

/// The daemon serving `local` once it is no longer instance `older`.
fn handed_over(local: &Path, older: u64, home: &Path) -> muster_daemon_proto::Welcome {
    // The window asks once the older daemon has sent its state, and the new one serves on the
    // same socket once the handoff is done.
    let deadline = std::time::Instant::now() + LAUNCH_PATIENCE;
    loop {
        let welcome =
            connection::connect(local, ConnectionKind::Control, "test").ok().map(|(_, hi)| hi);
        if let Some(welcome) = welcome.filter(|hi| hi.instance != older) {
            return welcome;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the older daemon was not handed over; the window's log is {}",
            home.join("muster.jsonl").display()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn type_line(input: &mut Input, text: &str) {
    let send = input_event::Send { text: text.to_string(), enter: true, ..Default::default() };
    input.send("p1", input_event::Input::Send(send));
}

/// The pid in the last `<what>=<pid>.` a pane shows. The command typed to print it shows `$$`
/// there instead.
fn said(text: &str, what: &str) -> Option<String> {
    text.lines().rev().find_map(|line| {
        let pid = line.rsplit_once(&format!("{what}="))?.1.strip_suffix('.')?;
        (!pid.is_empty() && pid.chars().all(|c| c.is_ascii_digit())).then(|| pid.to_string())
    })
}

/// Runs its closure when dropped, so the container is left as it was found even when an
/// assertion fails.
struct Finally<'a>(Box<dyn Fn() + 'a>);

impl Drop for Finally<'_> {
    fn drop(&mut self) {
        (self.0)();
    }
}

/// A home this test owns, so nothing here can resolve to a real one.
fn scratch_home() -> PathBuf {
    let path = PathBuf::from(format!("/tmp/muster-test/remote-handover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("the harness root should be writable");
    // SAFETY: the first thing the one test here does, so no thread of this binary has read the
    // environment yet. The seam reads it on the first request rather than at load.
    unsafe {
        std::env::set_var("MUSTER_HOME", &path);
    }
    path
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    Response::decode(muster::dispatch(&bytes).as_slice()).expect("a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
