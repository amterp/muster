//! The daemon started on a real remote machine, reached through a real ssh master.
//!
//! Out of the default gate, and marked `#[ignore]` to keep it there: it needs the devenv
//! container (`docs/testing.md`). `./dev --ssh` brings the container up and runs this, and the
//! container has no muster-daemon until this installs the one built from the same commit.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use muster_daemon_client::environment::for_far_daemon;
use muster_daemon_client::install::Carried;
use muster_daemon_client::launch::{Reached, stop};
use muster_daemon_client::remote::{Installed, ensure_running};
use muster_daemon_proto as proto;
use muster_harness::requests::*;
use muster_harness::{Control, DAEMON_DATA, Input, built_linux_daemons, until_some};
use muster_ssh::{Forward, Tunnel, remote_environment};

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
fn a_machine_with_no_daemon_gets_this_one_installed_started_and_then_adopted() {
    let (host, options) = devenv();
    let far = remote_environment(&host, &options).unwrap();
    let installed = Installed::on(&far).expect("the container has a HOME");
    let environment = for_far_daemon(&far);
    let temporary = std::env::temp_dir();
    let tunnel = Tunnel::open(
        Forward {
            host,
            options,
            control_path: temporary.join("muster-devenv-client.ctl").to_string_lossy().into(),
            local_socket: temporary.join("muster-devenv-client.sock").to_string_lossy().into(),
            remote_socket: installed.socket.to_string_lossy().into(),
            reverse: None,
        },
        Arc::new(|_| {}),
    )
    .expect("the tunnel opens");
    let local = Path::new(tunnel.local_socket_path());
    // One left from an earlier run is somebody's in real life; here it would make the start
    // below an adoption, and its install would make it no install at all.
    let _ = stop(local, Duration::from_secs(10));
    let far_home = installed.directory.parent().and_then(Path::parent).expect("under ~/.muster");
    tunnel
        .remote()
        .shell(&format!("rm -rf {}", muster_ssh::quoted(&far_home.to_string_lossy())))
        .unwrap();
    let carried = Carried {
        linux: Some(built_linux_daemons()),
        data: Some(DAEMON_DATA.into()),
        extras: Some(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras").into()),
        ..Carried::default()
    };

    let (reached, started) =
        ensure_running(&tunnel.remote(), &installed, &carried, local, &environment).unwrap();
    assert_eq!(reached, Reached::Started);
    let placed = tunnel
        .remote()
        .shell(&format!(
            "cd {} && test -f installed && test -d muster-daemon-data \
             && test -x muster-daemon-data/bin/ghostty \
             && test -x muster-daemon-data/bin/muster \
             && test -f {}/.claude-plugin/marketplace.json && echo placed",
            muster_ssh::quoted(&installed.directory.to_string_lossy()),
            muster_ssh::quoted(&installed.extras().to_string_lossy()),
        ))
        .unwrap();
    assert_eq!(
        placed.trim(),
        "placed",
        "the daemon was installed with its data and its stamp, and the scripts a pane runs for \
         `ssh` and `muster` are executable, and the adapters are linked where `muster harness \
         install` finds them"
    );

    let mut control = Control::connect(local);
    make(
        &mut control,
        proto::pane_request::Create {
            command: Some("echo over-there".into()),
            ..create("p1", in_new_tab("t1"))
        },
    );
    until_text(&mut control, "p1", "over-there");
    muster_is_on_the_path_over_there(&mut control, local);
    agents_message_each_other_over_there(&tunnel, &installed);
    make(&mut control, create("p4", in_new_tab("t4")));
    until_text(&mut control, "p4", "$");
    its_panes_are_driven_over_there_with_no_window(&tunnel, &installed);

    let (reached, adopted) =
        ensure_running(&tunnel.remote(), &installed, &carried, local, &environment).unwrap();
    assert_eq!(reached, Reached::Adopted);
    assert_eq!(adopted.instance, started.instance);

    stop(local, Duration::from_secs(10)).unwrap();
}

/// Debian's `/etc/profile` sets `PATH` outright for every login shell, which is what a pane's
/// shell is, so the `~/.muster/bin` the install links `muster` into is not on it. A command a
/// pane was made to run finds `muster` anyway, and so does a person typing at its prompt.
fn muster_is_on_the_path_over_there(control: &mut Control, local: &Path) {
    let version = format!("muster {}", env!("CARGO_PKG_VERSION"));
    make(
        control,
        proto::pane_request::Create {
            command: Some("muster --version".into()),
            ..create("p2", in_new_tab("t2"))
        },
    );
    until_text(control, "p2", &version);

    make(control, create("p3", in_new_tab("t3")));
    let mut input = Input::connect(local);
    // The integration adds the directory once the shell has drawn its first prompt, so a line
    // typed before that finds no `muster`; the loop types again until one does.
    until_some("muster typed at a prompt over there to answer", || {
        let send = proto::input_event::Send {
            text: "muster --version".into(),
            enter: true,
            ..Default::default()
        };
        input.send("p3", proto::input_event::Input::Send(send));
        std::thread::sleep(Duration::from_millis(500));
        let text = read_text(control, "p3", 0, 0).text;
        text.contains(&version).then_some(())
    });
}

/// The CLI the install linked into `~/.muster/bin` lists, types into and reads the panes of the
/// daemon over there, where no window is: its daemon answers in the window's place.
fn its_panes_are_driven_over_there_with_no_window(tunnel: &Tunnel, installed: &Installed) {
    let muster = muster_ssh::quoted(&installed.commands.join("muster").to_string_lossy());
    let said = tunnel
        .remote()
        .shell(&format!(
            "M={muster}; $M window; \
             $M pane send --pane p4 --enter --confirm 'echo typed-over-there'; echo \"sent=$?\"; \
             i=0; until $M pane read --pane p4 | grep -q '^typed-over-there' || [ $i -ge 100 ]; \
             do sleep 0.1; i=$((i+1)); done; $M pane read --pane p4 --rows 5"
        ))
        .unwrap();
    assert!(
        said.contains("no window answered; the muster-daemon at") && said.contains("p4"),
        "`muster window` over there lists the daemon's panes, saying the daemon answered: {said}"
    );
    assert!(said.contains("sent=0"), "a confirmed send over there reached its pane: {said}");
    assert!(
        said.lines().any(|line| line.trim() == "typed-over-there"),
        "the pane read over there shows what the send ran: {said}"
    );
}

/// The CLI the install linked into `~/.muster/bin` messages through the daemon over there: a
/// wait blocks in the daemon until a post wakes it, with nothing polling, and the guard refuses
/// a post on unread. A shell of its own, as an agent in a plain terminal on the devenv would be.
fn agents_message_each_other_over_there(tunnel: &Tunnel, installed: &Installed) {
    let muster = muster_ssh::quoted(&installed.commands.join("muster").to_string_lossy());
    let log = muster_ssh::quoted(&installed.socket.with_extension("log").to_string_lossy());
    let waited = muster_ssh::quoted(&installed.directory.join("waited").to_string_lossy());
    // The wait has to be in the daemon before the post, or the post is simply read later. The
    // daemon logs `msg.waiting` once it holds one, and the loop reads for that for at most
    // twenty seconds - a deadline on a condition, not a wait standing in for one.
    let said = tunnel
        .remote()
        .shell(&format!(
            "M={muster}; $M msg --as a join --group g && $M msg --as b join --group g && \
             {{ $M msg --as b wait --timeout 60 > {waited} 2>&1 & }}; \
             i=0; until grep -q msg.waiting {log} || [ $i -ge 200 ]; do sleep 0.1; i=$((i+1)); done; \
             grep -q msg.waiting {log} && echo blocked-before-the-post; \
             $M msg --as a post ping > /dev/null; wait; echo \"waited: $(cat {waited})\"; \
             $M msg --as b post too-soon 2>/dev/null; echo \"refused=$?\"; \
             $M msg --as b read > /dev/null; $M msg --as b post pong > /dev/null; echo \"posted=$?\""
        ))
        .unwrap();
    assert!(
        said.contains("blocked-before-the-post"),
        "the wait never reached the daemon before the post, so this shows nothing about a \
         blocked wait being woken: {said}"
    );
    assert!(
        said.contains("waited: [muster] g: 1 new (#4), from a. Read: muster msg read --group g"),
        "the wait was answered by the post: {said}"
    );
    assert!(said.contains("refused=1"), "the guard refused a post on unread: {said}");
    // 6 rather than 0 because a, a name given with --as, has no session to wake: the post was
    // kept, and exits saying nobody live heard it.
    assert!(said.contains("posted=6"), "reading cleared the way: {said}");
}
