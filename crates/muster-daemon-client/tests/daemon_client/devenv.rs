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
use muster_ssh::{Forward, Reverse, Tunnel, remote_environment};

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
    let laptop = Laptop::asleep(&temporary.join("muster-devenv-window.sock"));
    let tunnel = Tunnel::open(
        Forward {
            host,
            options,
            control_path: temporary.join("muster-devenv-client.ctl").to_string_lossy().into(),
            local_socket: temporary.join("muster-devenv-client.sock").to_string_lossy().into(),
            remote_socket: installed.socket.to_string_lossy().into(),
            reverse: Some(Reverse {
                remote_path: FAR_WINDOW.into(),
                local_path: laptop.socket.to_string_lossy().into(),
            }),
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
    with_the_laptop_asleep_quit_and_gone(&tunnel, &installed, &laptop);

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

/// Where the window answers over there: the path a laptop's Muster forwards its own socket to,
/// which every pane on the devenv is told as `$MUSTER_SOCKET`.
const FAR_WINDOW: &str = "/tmp/muster-devenv-window/window.sock";

/// The laptop's end of the window's forward: a socket that takes every connection and never
/// answers, which is what ssh on the devenv reaches while the laptop sleeps.
struct Laptop {
    socket: std::path::PathBuf,
}

impl Laptop {
    fn asleep(socket: &Path) -> Laptop {
        let _ = std::fs::remove_file(socket);
        let listener = std::os::unix::net::UnixListener::bind(socket).unwrap();
        std::thread::spawn(move || {
            // Held, not read: each connection stays open and nothing ever comes back.
            let mut held = Vec::new();
            for stream in listener.incoming() {
                held.push(stream);
            }
        });
        Laptop { socket: socket.to_path_buf() }
    }

    /// The window has quit with the laptop awake and the forward still up: ssh over there
    /// takes the connection, finds nothing listening here, and closes it.
    fn quit(&self) {
        std::fs::remove_file(&self.socket).unwrap();
    }
}

/// What an orchestrating agent on the devenv gets from `muster` while the laptop holding its
/// window sleeps, after the window quits, and once the forward is gone too: every verb it
/// spawns, drives and retires workers with is answered by the devenv's own daemon, and none
/// waits on the laptop longer than the two seconds a silent window is given.
fn with_the_laptop_asleep_quit_and_gone(tunnel: &Tunnel, installed: &Installed, laptop: &Laptop) {
    let muster = muster_ssh::quoted(&installed.commands.join("muster").to_string_lossy());
    let window = muster_ssh::quoted(FAR_WINDOW);
    // One line per command, `@@ <state> <verb> exit=<code> ms=<took>`, and what it said on
    // stderr after a failure, so a run that fails shows every verb's fate at once.
    let script = |state: &str| {
        format!(
            "M={muster}; export MUSTER_SOCKET={window}; S={state}; E=$(mktemp); \
             run() {{ v=$1; shift; s=$(date +%s%N); OUT=$(\"$@\" 2>$E); c=$?; \
               echo \"@@ $S $v exit=$c ms=$(( ($(date +%s%N) - s) / 1000000 ))\"; \
               [ $c -eq 0 ] || sed 's/^/   /' $E; }}; \
             run window $M window; \
             run new $M pane new --pane p4 --down --name worker-$S; NEW=$OUT; \
             run new-hidden $M pane new --pane p4 --hidden; HIDDEN=$OUT; \
             run rename $M pane rename --pane \"$NEW\" renamed-$S; \
             run send $M pane send --pane p4 --enter --confirm \"echo sent-$S\"; \
             run read $M pane read --pane p4 --rows 3; \
             run close $M pane close --pane \"$NEW\"; \
             run close-hidden $M pane close --pane \"$HIDDEN\"; \
             run msg-join $M msg --as lead-$S join --group crew-$S; \
             run msg-read $M msg --as lead-$S read --group crew-$S; \
             rm -f $E"
        )
    };
    // Asleep is the one state that costs a command anything: the two seconds a window is given
    // to answer before the daemon answers instead. Quit and gone are refused by the socket.
    let states: [(&str, u64, &dyn Fn()); 3] = [
        ("asleep", 8_000, &|| {}),
        ("quit", 4_000, &|| laptop.quit()),
        ("gone", 4_000, &|| {
            tunnel.remote().shell(&format!("rm -f {window}")).unwrap();
        }),
    ];
    for (state, within_ms, become_so) in states {
        become_so();
        let said = tunnel.remote().shell(&script(state)).unwrap();
        eprintln!("{said}");
        let outcomes: Vec<(&str, i32, u64)> = said
            .lines()
            .filter_map(|line| line.strip_prefix(&format!("@@ {state} ")))
            .filter_map(|rest| {
                let mut words = rest.split_whitespace();
                let verb = words.next()?;
                let code = words.next()?.strip_prefix("exit=")?.parse().ok()?;
                let took = words.next()?.strip_prefix("ms=")?.parse().ok()?;
                Some((verb, code, took))
            })
            .collect();
        assert_eq!(outcomes.len(), 10, "every verb ran with the laptop {state}:\n{said}");
        for (verb, code, took) in outcomes {
            assert!(
                code == 0 && took < within_ms,
                "with the laptop {state}, `{verb}` exited {code} after {took} ms, where the \
                 devenv's daemon should have answered within {within_ms} ms:\n{said}"
            );
        }
    }
}
