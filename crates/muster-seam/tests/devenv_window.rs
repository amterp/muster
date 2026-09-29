//! A program in a devenv pane drives the window that pane is drawn in.
//!
//! The window forwards its own socket to the devenv over the ssh master it already holds, and
//! tells every pane it makes there where that socket answers. So `muster pane new` typed into a
//! devenv pane makes a pane in the laptop's window, exactly as it does in a laptop pane.
//!
//! Out of the default gate, and marked `#[ignore]` to keep it there: it needs the devenv
//! container (`docs/testing.md`), which `./dev --ssh` brings up. Its own binary because it points
//! `MUSTER_HOME` at a scratch home before anything reads it, so the window's state files are
//! this test's rather than the developer's.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use muster::proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster::proto::{
    CreateTab, OpenWindow, Quitting, ReadPane, ReadWindow, Request, Response, SendToPane, Startup,
    Window, request, response,
};
use muster_daemon_client::launch::stop;
use muster_daemon_client::remote::Installed;
use muster_daemon_proto::launch::LAUNCH_PATIENCE;
use muster_harness::{DAEMON_DATA, built_linux_daemons, until_some, until_within};
use muster_ssh::remote_environment;
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
fn a_devenv_pane_drives_the_window_it_is_drawn_in() {
    let home = scratch_home();
    let (host, options) = devenv();
    let far = remote_environment(&host, &options).unwrap();
    let installed = Installed::on(&far).expect("the container has a HOME");
    let far_home = installed
        .directory
        .parent()
        .and_then(Path::parent)
        .expect("under ~/.muster")
        .to_string_lossy()
        .into_owned();
    let daemons = installed.socket.parent().expect("a socket is in a directory").to_owned();
    let local = std::env::temp_dir().join(format!("muster-{}-devenv.sock", std::process::id()));
    let control = std::env::temp_dir().join(format!("muster-{}-devenv.ctl", std::process::id()));
    let _cleaned = Finally(Box::new(|| {
        let _ = over_ssh(&host, &options, &format!("rm -rf {}", muster_ssh::quoted(&far_home)));
    }));

    let turn = muster::testing::fresh_session();
    let socket = home.join("command.sock");
    let log = home.join("muster.jsonl");
    open_a_window(&home, &socket, &log, &host, &options);

    // Asked until it lands, because the first ask can arrive while the window is still
    // installing and starting the devenv's daemon.
    let first = until_some("the devenv's daemon to make a tab", || {
        let asked = request::Payload::CreateTab(CreateTab {
            daemon_id: "devenv".to_string(),
            take_focus: true,
            ..CreateTab::default()
        });
        if let Some(response::Payload::Made(made)) = dialed(&socket, asked).payload {
            return Some(made.pane_id);
        }
        std::thread::sleep(Duration::from_millis(500));
        None
    });
    let _stopped = Finally(Box::new(|| {
        let _ = stop(&local, Duration::from_secs(10));
    }));
    until_shows(&socket, &log, "the tab it made", |window| on_devenv(window).contains(&first));

    // The laptop's side, as it was before any of this: a caller here reaches a devenv pane.
    type_into(&socket, &first, "echo said=$((40+2)).");
    until_within(
        "a read from the laptop to show what was typed into the devenv pane",
        LAUNCH_PATIENCE,
        || read_pane(&socket, &first).contains("said=42."),
        || format!("It shows:\n{}", read_pane(&socket, &first)),
    );

    // The devenv's side: its own `muster`, run in the pane, makes a pane in this window.
    let before = on_devenv(&read_window(&socket)).len();
    type_into(&socket, &first, "~/.muster/bin/muster pane new; echo made=$?.");
    until_shows(&socket, &log, "the pane the devenv pane asked for", |window| {
        on_devenv(window).len() == before + 1
    });
    until_within(
        "`muster` in the devenv pane to say it succeeded",
        LAUNCH_PATIENCE,
        || read_pane(&socket, &first).contains("made=0."),
        || format!("It shows:\n{}", read_pane(&socket, &first)),
    );

    the_daemon_answers_only_for_a_window_that_does_not(
        &socket, &host, &options, &installed, &first,
    );

    // The connection drops and comes back, and the same pane can still reach the window. What it
    // was told is a path on the devenv, and the reopened master forwards the window there again.
    let forwarded = forwards_so_far(&log);
    let ended = Command::new("ssh")
        .arg("-O")
        .arg("exit")
        .arg("-S")
        .arg(&control)
        .arg(&host)
        .output()
        .expect("ssh runs");
    assert!(ended.status.success(), "the master would not leave: {ended:?}");
    until_within(
        "the tunnel to reopen and forward the window again",
        LAUNCH_PATIENCE,
        || forwards_so_far(&log) > forwarded,
        || format!("The window's log is {}", log.display()),
    );
    // The daemon's own connection rides the same master and comes back on its own schedule,
    // and typing into the pane needs it.
    until_shows(&socket, &log, "the devenv connected again", |window| {
        window
            .daemons
            .iter()
            .any(|machine| machine.daemon_id == "devenv" && machine.state == "connected")
    });
    let before = on_devenv(&read_window(&socket)).len();
    type_into(&socket, &first, "~/.muster/bin/muster pane new; echo again=$?.");
    until_shows(&socket, &log, "the pane asked for after the reconnect", |window| {
        on_devenv(window).len() == before + 1
    });

    // Quitting ends the window's master, and its forwards with it: nothing is left running
    // here for launchd to inherit, still carrying this window's socket to the devenv, where a
    // pane asking for its window would reach one that has quit (kan a_2YAdjRtMB). Asked to end
    // the session as well, so the devenv's daemon is stopped through the master first.
    assert_ok(&answer(request::Payload::Quitting(Quitting { close_sessions: true })));
    let checked = Command::new("ssh")
        .arg("-O")
        .arg("check")
        .arg("-S")
        .arg(&control)
        .arg(&host)
        .output()
        .expect("ssh runs");
    assert!(!checked.status.success(), "the window's ssh master outlived its quit: {checked:?}");

    // And its socket is off the devenv, so what is left there does not look like a window that
    // might answer.
    drop(turn);
    drop(muster::testing::fresh_session());
    let left = over_ssh(
        &host,
        &options,
        &format!(
            "ls {}/window-*.sock 2>/dev/null || true",
            muster_ssh::quoted(&daemons.to_string_lossy())
        ),
    );
    assert!(left.trim().is_empty(), "the window's socket outlived it on the devenv: {left}");
}

/// A devenv pane's `muster window` is answered by the window while the window answers there. A
/// pane whose `$MUSTER_SOCKET` names a window of this Muster that has quit - Muster relaunched -
/// is answered by the window open now, which forwards beside it; and one naming a window no
/// window of this Muster is beside is answered by the devenv's own daemon.
fn the_daemon_answers_only_for_a_window_that_does_not(
    socket: &Path,
    host: &str,
    options: &[String],
    installed: &Installed,
    first: &str,
) {
    // With the window answering there, it answers rather than the devenv's daemon: its
    // listing is a window's, not headed by the daemon that answered in a window's place.
    type_into(
        socket,
        first,
        "echo daemon=$(~/.muster/bin/muster window | grep -c 'no window answered').",
    );
    until_within(
        "`muster window` in the devenv pane to be answered",
        LAUNCH_PATIENCE,
        || read_pane(socket, first).contains("daemon=0."),
        || format!("It shows:\n{}", read_pane(socket, first)),
    );
    // A window of this Muster that has quit: the window open now answers, and a change from the
    // pane reaches it.
    let daemons = installed.socket.parent().expect("a socket is in a directory");
    let install = installed.socket.file_stem().expect("a socket has a name").to_string_lossy();
    let relaunched = daemons.join(format!("window-{install}-wgone.sock"));
    let said = over_ssh(
        host,
        options,
        &format!(
            "export MUSTER_SOCKET={} MUSTER_DAEMON_SOCKET={} MUSTER_PANE={first}; \
             ~/.muster/bin/muster window | grep -c 'no window answered'; \
             ~/.muster/bin/muster pane rename 'reached'; echo renamed=$?.",
            muster_ssh::quoted(&relaunched.to_string_lossy()),
            muster_ssh::quoted(&installed.socket.to_string_lossy()),
        ),
    );
    assert!(
        said.starts_with('0') && said.contains("renamed=0."),
        "a pane whose window relaunched did not reach the window open now: {said}"
    );

    // A window of another Muster forwarding here, or none: the devenv's own daemon answers.
    let gone = daemons.join("window-another-install-wgone.sock");
    let said = over_ssh(
        host,
        options,
        &format!(
            "export MUSTER_SOCKET={} MUSTER_DAEMON_SOCKET={}; ~/.muster/bin/muster window; \
             ~/.muster/bin/muster pane read --pane {first}",
            muster_ssh::quoted(&gone.to_string_lossy()),
            muster_ssh::quoted(&installed.socket.to_string_lossy()),
        ),
    );
    assert!(
        said.contains("no window answered; the muster-daemon at") && said.contains(first),
        "with its window gone, a devenv pane's `muster window` is answered by its daemon: {said}"
    );
    assert!(said.contains("said=42."), "and `pane read` reads the pane from there: {said}");
}

/// Opens a window configured with the devenv's daemon and nothing else, listening on `socket`.
fn open_a_window(home: &Path, socket: &Path, log: &Path, host: &str, options: &[String]) {
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
            log_path: log.to_string_lossy().into_owned(),
            command_socket_path: socket.to_string_lossy().into_owned(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow {}),
    ] {
        assert_ok(&answer(payload));
    }
}

/// How many times the window's log says it forwarded itself to a machine.
fn forwards_so_far(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains("\"tunnel.reverse\""))
        .count()
}

fn until_shows(socket: &Path, log: &Path, what: &str, ready: impl Fn(&Window) -> bool) {
    until_within(
        &format!("the window to show {what}"),
        LAUNCH_PATIENCE,
        || ready(&read_window(socket)),
        || format!("It shows {:?}. The window's log is {}", read_window(socket), log.display()),
    );
}

/// Muster's names for the panes the window shows on the devenv.
fn on_devenv(window: &Window) -> Vec<String> {
    window
        .roster
        .iter()
        .flat_map(|roster| roster.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .filter(|pane| pane.daemon_id == "devenv")
        .map(|pane| pane.pane_id.clone())
        .collect()
}

fn type_into(socket: &Path, pane: &str, text: &str) {
    assert_ok(&dialed(
        socket,
        request::Payload::SendToPane(SendToPane {
            pane_id: pane.to_string(),
            text: text.to_string(),
            enter: true,
            ..SendToPane::default()
        }),
    ));
}

fn read_pane(socket: &Path, pane: &str) -> String {
    match dialed(
        socket,
        request::Payload::ReadPane(ReadPane { pane_id: pane.to_string(), ..ReadPane::default() }),
    )
    .payload
    {
        Some(response::Payload::PaneText(text)) => text.text,
        other => panic!("the window answered a ReadPane with {other:?}"),
    }
}

fn read_window(socket: &Path) -> Window {
    match dialed(socket, request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("the window answered a ReadWindow with {other:?}"),
    }
}

/// One request over the window's socket, the way the laptop's `muster` sends it.
fn dialed(socket: &Path, payload: request::Payload) -> Response {
    let mut stream = UnixStream::connect(socket).expect("the window is listening");
    let asking = Request { payload: Some(payload) }.encode_to_vec();
    write_frame(&mut stream, &asking).expect("the window takes a request");
    let reply = read_frame(&mut stream, LARGEST_MESSAGE).expect("the window answers it");
    Response::decode(reply.as_slice()).expect("an answer this build knows")
}

/// A shell script on the devenv over a connection of its own, since the window's may be gone.
fn over_ssh(host: &str, options: &[String], script: &str) -> String {
    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes"])
        .args(options)
        .arg(host)
        .arg("sh")
        .arg("-c")
        .arg(muster_ssh::quoted(script))
        .output()
        .expect("ssh runs");
    String::from_utf8_lossy(&output.stdout).into_owned()
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
    let path = PathBuf::from(format!("/tmp/muster-test/devenv-window-{}", std::process::id()));
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
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    Response::decode(muster::dispatch(&bytes).as_slice()).expect("a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the window refused: {}", failure.reason);
    }
}
