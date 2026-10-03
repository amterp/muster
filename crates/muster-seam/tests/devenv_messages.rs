//! An agent on the laptop and one on the devenv, neither in a pane, share one group.
//!
//! A window attached to both machines links the laptop's daemon to the devenv's over the ssh
//! connection it already holds (MIP-4, section 11, decision 4a), so a group made on the laptop
//! can be joined from the devenv, a post on either wakes the other, and the stale-context guard
//! holds across the link. With the window closed the link is gone: a post to the laptop's group
//! from the devenv fails at once, naming the laptop, while a group kept on the devenv posts as
//! ever; and what was posted meanwhile arrives once a window links them again.
//!
//! The devenv's agent is `~/.muster/bin/muster` over ssh, as the window installed it. The
//! laptop's is this test sending the requests `muster msg` sends, to the laptop's daemon.
//!
//! Out of the default gate, and marked `#[ignore]` to keep it there: it needs the devenv
//! container (`docs/testing.md`), which `./dev --ssh` brings up. Its own binary because it points
//! `MUSTER_HOME` at a scratch home before anything reads it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use muster::proto::{
    OpenWindow, ReadWindow, Request, Response, Startup, Window, request, response,
};
use muster_daemon_client::launch::stop;
use muster_daemon_client::remote::Installed;
use muster_daemon_proto as proto;
use muster_daemon_proto::launch::LAUNCH_PATIENCE;
use muster_harness::requests::{expect, session};
use muster_harness::{Control, DAEMON_DATA, Daemon, built_linux_daemons, until_within};
use muster_ssh::remote_environment;
use prost::Message;
use proto::msg_answer::{self, Answer};
use proto::msg_request::{self, Request as Asked};
use proto::request::Service;

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
fn a_laptop_agent_and_a_devenv_agent_share_a_group() {
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
    let far_log = installed.socket.with_extension("log").to_string_lossy().into_owned();
    let forwarded = std::env::temp_dir().join(format!("muster-{}-devenv.sock", std::process::id()));
    let _cleaned = Finally(Box::new(|| {
        let _ = over_ssh(&host, &options, &format!("rm -rf {}", muster_ssh::quoted(&far_home)));
    }));
    let _stopped = Finally(Box::new(|| {
        let _ = stop(&forwarded, Duration::from_secs(10));
    }));
    let laptop = Daemon::start_built();
    let mut near = laptop.connect();
    let mut near_log = following(&laptop);
    let devenv = |script: &str| {
        let script =
            format!("M=~/.muster/bin/muster; F={}; {script}", muster_ssh::quoted(&far_log));
        over_ssh(&host, &options, &script)
    };

    let turn = muster::testing::fresh_session();
    let socket = home.join("command.sock");
    let log = home.join("muster.jsonl");
    open_a_window(&home, &socket, &log, &laptop, &host, &options);
    until_linked(&mut near_log, 1, &log);

    // A group made on the laptop, joined from the devenv by its name alone.
    assert_eq!(join(&mut near, "builder", "review"), "review");
    let joined = devenv("$M msg --as critic join --group review");
    let there = joined
        .split_whitespace()
        .find(|word| word.starts_with("review@"))
        .unwrap_or_else(|| panic!("the devenv joined the laptop's review: {joined}"))
        .to_string();

    // A post on the laptop wakes the devenv's agent from its wait.
    let waited = devenv(
        "$M msg --as critic wait --timeout 60 > ~/waited 2>&1 & \
         i=0; until grep -q msg.waiting \"$F\" || [ $i -ge 100 ]; do sleep 0.1; i=$((i+1)); done; \
         echo ready",
    );
    assert!(waited.contains("ready"), "{waited}");
    let posted = expect(&mut near, post("builder", None, &[], "rebase first"), Done);
    assert_eq!(reached(&posted), [("critic@devenv".to_string(), msg_answer::Reach::Woken)]);
    until_within(
        "the devenv's wait to be answered",
        LAUNCH_PATIENCE,
        || devenv("cat ~/waited").contains(&format!("[muster] {there}")),
        || devenv("cat ~/waited"),
    );

    // The guard holds across: the devenv's post waits for its read.
    let said = devenv(
        "$M msg --as critic post 'pushed'; echo refused=$?; $M msg --as critic read >/dev/null; \
         $M msg --as critic post 'rebased, then pushed'; echo posted=$?",
    );
    assert!(said.contains("refused=1") && said.contains("unread"), "{said}");
    assert!(said.contains("posted=6"), "appended, and heard by nobody with an inbox: {said}");
    assert_eq!(read(&mut near, "builder"), ["critic@devenv: rebased, then pushed"]);

    // The ssh connection drops and comes back on its own; the link comes back with it.
    let control = std::env::temp_dir().join(format!("muster-{}-devenv.ctl", std::process::id()));
    let ended = Command::new("ssh")
        .args(["-O", "exit", "-S"])
        .arg(&control)
        .arg(&host)
        .output()
        .expect("ssh runs");
    assert!(ended.status.success(), "the master would not leave: {ended:?}");
    until_linked(&mut near_log, 2, &log);
    let said = devenv("$M msg --as critic post 'after the drop'; echo posted=$?");
    assert!(said.contains("posted=6"), "{said}");

    // With the window closed nothing links the two, and a post across fails at once.
    devenv("$M msg --as critic join --group desk && $M msg --as scout join --group desk");
    drop(turn);
    let turn = muster::testing::fresh_session();
    let said = devenv(
        "$M msg --as critic post --group review 'anyone?'; echo refused=$?; \
         $M msg --as critic post --group desk 'still here'; echo desk=$?",
    );
    let laptop_name = there.trim_start_matches("review@");
    assert!(said.contains("refused=1") && said.contains(laptop_name), "{said}");
    assert!(said.contains("desk=6"), "a group kept on the devenv still posts: {said}");
    assert_eq!(read(&mut near, "builder"), ["critic@devenv: after the drop"]);
    let kept = expect(&mut near, post("builder", None, &["critic"], "while you were away"), Done);
    assert_eq!(reached(&kept), [("critic@devenv".to_string(), msg_answer::Reach::Unreachable)]);

    // A window links them again, and what the laptop's group took meanwhile arrives.
    open_a_window(&home, &socket, &log, &laptop, &host, &options);
    until_linked(&mut near_log, 3, &log);
    until_within(
        "the devenv to read what was posted while the window was closed",
        LAUNCH_PATIENCE,
        || devenv("$M msg --as critic read").contains("while you were away"),
        || devenv("$M msg --as critic log --group review"),
    );
    let _ = stop(&forwarded, Duration::from_secs(10));
    // A new session is what closes the old window, and its ssh master with it.
    drop(turn);
    drop(muster::testing::fresh_session());
}

use proto::Outcome::Done;

fn named(name: &str) -> msg_request::Caller {
    msg_request::Caller { as_name: Some(name.to_string()), ..msg_request::Caller::default() }
}

fn msg(caller: &str, asked: Asked) -> Service {
    Service::Msg(proto::MsgRequest { caller: Some(named(caller)), request: Some(asked) })
}

fn msg_answer(asked: &muster_harness::Asked) -> &proto::MsgAnswer {
    match &asked.answer.detail {
        Some(proto::answer::Detail::Msg(answer)) => answer,
        other => panic!("a msg request answered with {other:?}: {}", asked.answer.reason),
    }
}

fn join(control: &mut Control, name: &str, group: &str) -> String {
    let asked = Asked::Join(msg_request::Join {
        name: Some(name.to_string()),
        group: Some(group.to_string()),
        pull: false,
    });
    match &msg_answer(&expect(control, msg(name, asked), Done)).answer {
        Some(Answer::Joined(joined)) => joined.group.clone().unwrap_or_default(),
        other => panic!("a join answered with {other:?}"),
    }
}

fn post(author: &str, group: Option<&str>, to: &[&str], body: &str) -> Service {
    let asked = Asked::Post(msg_request::Post {
        group: group.map(str::to_string),
        to: to.iter().map(|name| (*name).to_string()).collect(),
        body: body.to_string(),
        urgent: false,
    });
    msg(author, asked)
}

fn reached(asked: &muster_harness::Asked) -> Vec<(String, msg_answer::Reach)> {
    match &msg_answer(asked).answer {
        Some(Answer::Posted(posted)) => {
            posted.reached.iter().map(|reached| (reached.name.clone(), reached.reach())).collect()
        }
        other => panic!("a post answered with {other:?}"),
    }
}

/// The messages `name` had not read, as `author: body`.
fn read(control: &mut Control, name: &str) -> Vec<String> {
    let asked = expect(control, msg(name, Asked::Read(msg_request::Read { group: None })), Done);
    let Some(Answer::Entries(entries)) = &msg_answer(&asked).answer else {
        panic!("expected entries, got {:?}", msg_answer(&asked));
    };
    entries
        .groups
        .iter()
        .flat_map(|group| &group.entries)
        .filter_map(|entry| match &entry.what {
            Some(msg_answer::entry::What::Message(message)) => {
                Some(format!("{}: {}", message.author, message.body))
            }
            _ => None,
        })
        .collect()
}

fn following(daemon: &Daemon) -> Control {
    let mut logging = daemon.connect();
    let follow = proto::session_request::Request::FollowLog(proto::session_request::FollowLog {
        after: None,
    });
    expect(&mut logging, session(follow), Done);
    logging
}

/// Waits for the laptop's daemon to have linked to the devenv's `times` times in all.
fn until_linked(near_log: &mut Control, times: usize, log: &Path) {
    let lines = near_log.logged_times_until("msg.peer.linked", times, LAUNCH_PATIENCE * 2);
    let count = lines.iter().filter(|line| line.line.contains("msg.peer.linked")).count();
    assert_eq!(
        count,
        times,
        "the window linked the laptop's daemon to the devenv's; the window's log is {}",
        log.display()
    );
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
        request::Payload::OpenWindow(OpenWindow {}),
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
    let asking = Request { payload: Some(request::Payload::ReadWindow(ReadWindow {})) };
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
    let path = PathBuf::from(format!("/tmp/muster-test/devenv-messages-{}", std::process::id()));
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
