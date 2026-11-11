//! A tab interleaving the laptop's panes with the devenv's comes back whole when the devenv's
//! daemon is restarted under it.
//!
//! The tab is `columns(A@local, rows(X@devenv, B@local))`: the devenv's one pane sits between two
//! of the laptop's, so neither machine's part is a block the other's could be put back beside.
//! Each daemon keeps the whole tree as its copy of the tab (MIP-8). The devenv's daemon is told
//! to stop, and the window starts it again over ssh, as it does for any daemon it manages that
//! has gone. The restarted daemon reads its copy back from its own state, so X returns to its
//! place, and neither copy is written without the other machine's panes on the way.
//!
//! Out of the default gate, and marked `#[ignore]` to keep it there: it needs the devenv
//! container (`docs/testing.md`), which `./dev --ssh` brings up. Its own binary because it points
//! `MUSTER_HOME` at a scratch home before anything reads it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use muster::proto::{
    CreateTab, OpenWindow, ReadPane, ReadWindow, Request, Response, SendToPane, SplitPane, Startup,
    ViewTree, Window, request, response, view_tree,
};
use muster_daemon_client::launch::stop;
use muster_daemon_client::remote::Installed;
use muster_daemon_proto::launch::LAUNCH_PATIENCE;
use muster_harness::requests::{shape, snapshot};
use muster_harness::{Control, DAEMON_DATA, Daemon, built_linux_daemons, until_some, until_within};
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
fn a_tab_interleaving_the_devenv_comes_back_whole_when_its_daemon_restarts() {
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
    // Where the window forwards the devenv's daemon to on this machine.
    let forwarded = std::env::temp_dir().join(format!("muster-{}-devenv.sock", std::process::id()));
    let _cleaned = Finally(Box::new(|| {
        let _ = over_ssh(&host, &options, &format!("rm -rf {}", muster_ssh::quoted(&far_home)));
    }));
    let _stopped = Finally(Box::new(|| {
        let _ = stop(&forwarded, Duration::from_secs(10));
    }));
    let laptop = Daemon::start_built();

    let turn = muster::testing::fresh_session();
    let socket = home.join("command.sock");
    let log = home.join("muster.jsonl");
    open_a_window(&home, &socket, &log, &laptop, &host, &options);

    let a = made(&request::Payload::CreateTab(CreateTab {
        daemon_id: "local".to_string(),
        take_focus: true,
        ..CreateTab::default()
    }));
    let x = split(&a, "right", "devenv");
    let b = split(&x, "down", "local");
    let whole = format!("columns({a}@local, rows({x}@devenv, {b}@local))");
    let tab = until_some("the window to draw the tab interleaving both machines", || {
        let window = read_window(&socket);
        window
            .layouts
            .iter()
            .find(|layout| layout.tree.as_ref().is_some_and(|tree| shape_of(tree) == whole))
            .map(|layout| layout.tab_id.clone())
    });
    let copy = format!("[{a}|[~{x}@devenv/{b} 0.50] 0.50]");
    let devenv_copy = format!("[~{a}@local|[{x}/~{b}@local 0.50] 0.50]");
    until_within(
        "both daemons to keep the whole tree as their copy of the tab",
        LAUNCH_PATIENCE,
        || {
            copy_of(&mut laptop.connect(), &tab).is_some_and(|(tree, _)| tree == copy)
                && copy_of(&mut Control::connect(&forwarded), &tab)
                    .is_some_and(|(tree, _)| tree == devenv_copy)
        },
        || {
            format!(
                "the laptop holds {:?} and the devenv {:?}",
                copy_of(&mut laptop.connect(), &tab),
                copy_of(&mut Control::connect(&forwarded), &tab)
            )
        },
    );
    let (_, laptop_generation) = copy_of(&mut laptop.connect(), &tab).expect("waited for it");

    // The devenv's daemon goes, with the window still open, and the window starts it again.
    stop(&forwarded, Duration::from_secs(10)).expect("the devenv's daemon stops when asked");
    until_within(
        "the window to start the devenv's daemon again",
        LAUNCH_PATIENCE * 2,
        || logged(&log, "daemon.started_again"),
        || format!("The window's log is {}", log.display()),
    );
    until_within(
        "the window to draw the tab as it was, with X back between A and B",
        LAUNCH_PATIENCE,
        || {
            let window = read_window(&socket);
            let connected = window
                .daemons
                .iter()
                .any(|machine| machine.daemon_id == "devenv" && machine.state == "connected");
            connected && drawn(&window, &tab).is_some_and(|tree| tree == whole)
        },
        || {
            format!(
                "It draws {:?}. The window's log is {}",
                drawn(&read_window(&socket), &tab),
                log.display()
            )
        },
    );

    // Neither copy lost the other machine's panes on the way, and the laptop's needed nothing
    // written: the restarted devenv brought its own copy back from its state.
    let (laptop_tree, generation) = copy_of(&mut laptop.connect(), &tab).expect("the laptop's");
    assert_eq!(laptop_tree, copy, "the laptop's copy keeps X between A and B");
    assert_eq!(generation, laptop_generation, "and nothing was written to it");
    let (devenv_tree, _) =
        copy_of(&mut Control::connect(&forwarded), &tab).expect("the restarted devenv's");
    assert_eq!(devenv_tree, devenv_copy, "the restarted devenv's copy keeps A and B around X");

    // And X is a pane again rather than a place for one: its new shell answers.
    send(&x, "echo back=$((40+2)).");
    until_within(
        "the restarted devenv's pane to run what was typed",
        LAUNCH_PATIENCE,
        || read_pane(&x).contains("back=42."),
        || format!("It shows:\n{}", read_pane(&x)),
    );

    let _ = stop(&forwarded, Duration::from_secs(10));
    drop(turn);
    drop(muster::testing::fresh_session());
}

/// Splits `pane`, putting the new pane on `side` of it and on `machine`.
fn split(pane: &str, side: &str, machine: &str) -> String {
    made(&request::Payload::SplitPane(SplitPane {
        pane_id: pane.to_string(),
        side: side.to_string(),
        new_pane_daemon_id: machine.to_string(),
        ..SplitPane::default()
    }))
}

/// The pane a request made, asked until it lands: a first ask can arrive while the window is
/// still starting the devenv's daemon.
fn made(payload: &request::Payload) -> String {
    until_some("the window to make the pane", || {
        if let Some(response::Payload::Made(made)) = answer(payload.clone()).payload {
            return Some(made.pane_id);
        }
        std::thread::sleep(Duration::from_millis(500));
        None
    })
}

/// A daemon's copy of `tab` as [`shape`] writes it, and the generation it was written at.
fn copy_of(control: &mut Control, tab: &str) -> Option<(String, u64)> {
    let held = snapshot(control).tabs.into_iter().find(|held| held.tab == tab)?;
    Some((shape(held.root.as_ref()?), held.layout_generation))
}

fn drawn(window: &Window, tab: &str) -> Option<String> {
    let layout = window.layouts.iter().find(|layout| layout.tab_id == tab)?;
    Some(shape_of(layout.tree.as_ref()?))
}

/// A tree the window draws, as text and without its ratios: `columns(p1@local, p2@devenv)`.
fn shape_of(tree: &ViewTree) -> String {
    match &tree.node {
        Some(view_tree::Node::Pane(leaf)) => {
            let pane = leaf.pane.as_ref().map_or("?", |pane| pane.pane_id.as_str());
            format!("{pane}@{}", leaf.daemon_id)
        }
        Some(view_tree::Node::Unreachable(leaf)) => format!("?{}@{}", leaf.pane_id, leaf.daemon_id),
        Some(view_tree::Node::Split(split)) => {
            let child =
                |child: &Option<Box<ViewTree>>| child.as_deref().map_or_else(String::new, shape_of);
            format!("{}({}, {})", split.axis, child(&split.first), child(&split.second))
        }
        None => String::new(),
    }
}

fn logged(log: &Path, event: &str) -> bool {
    std::fs::read_to_string(log).is_ok_and(|text| text.contains(&format!("\"{event}\"")))
}

fn send(pane: &str, text: &str) {
    assert_ok(&answer(request::Payload::SendToPane(SendToPane {
        pane_id: pane.to_string(),
        text: text.to_string(),
        enter: true,
        ..SendToPane::default()
    })));
}

fn read_pane(pane: &str) -> String {
    match answer(request::Payload::ReadPane(ReadPane {
        pane_id: pane.to_string(),
        ..ReadPane::default()
    }))
    .payload
    {
        Some(response::Payload::PaneText(text)) => text.text,
        other => format!("no text: {other:?}"),
    }
}

/// Opens a window configured with the laptop's daemon and the devenv's.
fn open_a_window(
    home: &Path,
    socket: &Path,
    log: &Path,
    laptop: &Daemon,
    host: &str,
    options: &[String],
) {
    let config = home.join("config.toml");
    let quoted: Vec<String> = options.iter().map(|option| format!("{option:?}")).collect();
    std::fs::write(
        &config,
        format!(
            "[[daemon]]\nid = \"local\"\nsocket = {:?}\n\n\
             [[daemon]]\nid = \"devenv\"\nhost = {host:?}\nssh_options = [{}]\n",
            laptop.socket_path().to_string_lossy(),
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
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        assert_ok(&answer(payload));
    }
    until_within(
        "the window to attach both daemons",
        LAUNCH_PATIENCE * 2,
        || {
            let window = read_window(socket);
            ["local", "devenv"].iter().all(|id| {
                window
                    .daemons
                    .iter()
                    .any(|machine| machine.daemon_id == *id && machine.state == "connected")
            })
        },
        || format!("It shows {:?}. The window's log is {}", read_window(socket), log.display()),
    );
}

fn read_window(socket: &Path) -> Window {
    use muster::proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
    let mut stream = std::os::unix::net::UnixStream::connect(socket).expect("the window listens");
    let asking = Request::new(request::Payload::ReadWindow(ReadWindow { layout: true }));
    write_frame(&mut stream, &asking.encode_to_vec()).expect("the window takes a request");
    let reply = read_frame(&mut stream, LARGEST_MESSAGE).expect("the window answers it");
    match Response::decode(reply.as_slice()).expect("an answer this build knows").payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("the window answered a ReadWindow with {other:?}"),
    }
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
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
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
    let path = PathBuf::from(format!("/tmp/muster-test/devenv-spanning-{}", std::process::id()));
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
        panic!("the window refused: {}", failure.reason);
    }
}
