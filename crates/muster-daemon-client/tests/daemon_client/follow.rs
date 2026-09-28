//! Following a daemon as the core's backend, against the real daemon.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use muster_core::config::{ClipboardWrite, Cursor, CursorStyle, Shell, ShellMode};
use muster_core::daemon_settings::DaemonSettings;
use muster_core::input::NotSent;
use muster_core::intent::{BackendChannel, BackendIntent, Side};
use muster_core::mirror::Mirror;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use muster_core::mirror::backend::{PaneId, TabId};

static NEXT_TAB: AtomicU32 = AtomicU32::new(10);
use muster_core::names::{Mint, Minter};
use muster_daemon_client::backend::DaemonBackend;
use muster_daemon_client::follow::{Follower, Following, Notice};
use muster_daemon_client::records;
use muster_daemon_proto as proto;
use muster_harness::requests::{snapshot, until_text};
use muster_harness::{Daemon, until, until_some};

struct Followed {
    follower: Follower,
    mirror: Arc<Mutex<Mirror>>,
    notices: Arc<Mutex<Vec<Notice>>>,
    backend: DaemonBackend,
}

fn follow(daemon: &Daemon) -> Followed {
    let mirror = Arc::new(Mutex::new(Mirror::new()));
    let notices = Arc::new(Mutex::new(Vec::new()));
    let heard = Arc::clone(&notices);
    let follower = Follower::start(
        Following {
            socket: daemon.socket_path().to_path_buf(),
            client: "test".to_string(),
            daemon: "local".to_string(),
            remote: false,
        },
        Arc::clone(&mirror),
        Arc::new(move |notice| heard.lock().unwrap().push(notice)),
    )
    .unwrap();
    let backend = DaemonBackend::new(
        follower.connection(),
        Arc::clone(&mirror),
        Arc::new(Mutex::new(Minter::new(Mint::Drawn))),
        BTreeMap::from([("MUSTER_SOCKET".to_string(), "/tmp/window.sock".to_string())]),
        "the test daemon".to_string(),
    );
    let state = Followed { follower, mirror, notices, backend };
    state.bootstrapped(1);
    state
}

impl Followed {
    /// Waits until the mirror has been bootstrapped this many times.
    fn bootstrapped(&self, times: usize) {
        until_some(&format!("{times} bootstrap(s)"), || {
            let heard = self.notices.lock().unwrap();
            let count = heard.iter().filter(|n| matches!(n, Notice::Bootstrapped { .. })).count();
            (count >= times).then_some(())
        });
    }
}

/// Everything a request did arrives before its answer, so a submit returns with its effect in
/// the mirror and nothing has to wait for it.
#[test]
fn a_request_has_taken_effect_in_the_mirror_by_the_time_it_returns() {
    let daemon = Daemon::start_built();
    let followed = follow(&daemon);

    let made = followed
        .backend
        .submit(&BackendIntent::CreateTab {
            tab: TabId::new("t1"),
            cwd: None,
            run: None,
            name: Some("first".into()),
        })
        .unwrap();
    let (pane, tab) = (made.created.unwrap(), TabId::new("t1"));
    {
        let mirror = followed.mirror.lock().unwrap();
        let held = mirror.pane(&pane).expect("the new pane is in the mirror already");
        assert_eq!(held.tab, tab);
        assert_eq!(held.name.as_deref(), Some("first"));
    }

    let split = followed
        .backend
        .submit(&BackendIntent::SplitPane {
            pane: pane.clone(),
            side: Side::Left,
            ratio: None,
            cwd: None,
            run: None,
            name: None,
        })
        .unwrap()
        .created
        .unwrap();
    let mirror = followed.mirror.lock().unwrap();
    let tree = mirror.tree(&tab).unwrap().to_string();
    assert_eq!(tree, format!("columns({split}, {pane}@0.5)"), "the new pane is on the left");
}

/// A zoom is a toggle to the window and a state to the daemon, read from the mirror.
#[test]
fn zooming_twice_puts_the_tab_back() {
    let daemon = Daemon::start_built();
    let followed = follow(&daemon);
    let made = followed.backend.submit(&BackendIntent::CreateTab {
        tab: TabId::new(format!("t{}", NEXT_TAB.fetch_add(1, Ordering::Relaxed))),
        cwd: None,
        run: None,
        name: None,
    });
    let pane = made.unwrap().created.unwrap();
    let zoomed = |followed: &Followed, pane: &PaneId| {
        let mirror = followed.mirror.lock().unwrap();
        mirror.tab(&mirror.pane(pane).unwrap().tab).unwrap().zoomed.clone()
    };
    followed.backend.submit(&BackendIntent::ZoomPane { pane: pane.clone() }).unwrap();
    assert_eq!(zoomed(&followed, &pane), Some(pane.clone()));
    followed.backend.submit(&BackendIntent::ZoomPane { pane: pane.clone() }).unwrap();
    assert_eq!(zoomed(&followed, &pane), None);
}

/// A pane holding more history than one answer carries reads back ending at its newest row,
/// and says the older rows were left out. The newest rows are what a read is for: `--rows N`
/// takes its tail, and an agent reads a neighbour to see what it just did.
#[test]
fn a_read_longer_than_one_answer_ends_at_the_newest_row() {
    let daemon = Daemon::start_built();
    let followed = follow(&daemon);
    let settings = DaemonSettings { scrollback_bytes: Some(1 << 30), ..DaemonSettings::default() };
    followed.follower.configure(&settings);
    let mut control = daemon.connect();
    until_some("the scrollback setting to arrive", || {
        let settings = snapshot(&mut control).settings.unwrap_or_default();
        (settings.scrollback_bytes == Some(1 << 30)).then_some(())
    });

    // About 5.7 MB of text, past the daemon's 4 MiB page, in rows too short to wrap.
    let run = "awk 'BEGIN { for (i = 0; i < 80000; i++) printf \"%070d\\n\", i; \
               print \"THE-END\" }'";
    let made = followed.backend.submit(&BackendIntent::CreateTab {
        tab: TabId::new(format!("t{}", NEXT_TAB.fetch_add(1, Ordering::Relaxed))),
        cwd: None,
        run: Some(run.into()),
        name: None,
    });
    let pane = made.unwrap().created.unwrap();

    let read = until_some("the pane's last row to be read back", || {
        let read = followed.backend.read(&pane).unwrap();
        read.text.contains("THE-END").then_some(read)
    });
    assert!(read.truncated, "a read that left out the oldest rows has to say so");
    assert!(
        !read.text.contains(&format!("{:070}\n", 0)),
        "the read began at the oldest row, so it holds more than one answer can"
    );
}

/// A daemon that died and came back is followed again from a fresh snapshot, and the window
/// was told it went stale in between.
#[test]
fn a_daemon_that_comes_back_is_followed_again() {
    let mut daemon = Daemon::start_built();
    let followed = follow(&daemon);
    followed
        .backend
        .submit(&BackendIntent::CreateTab {
            tab: TabId::new(format!("t{}", NEXT_TAB.fetch_add(1, Ordering::Relaxed))),
            cwd: None,
            run: None,
            name: None,
        })
        .unwrap();

    daemon.kill();
    until_some("the window told the daemon went stale", || {
        let heard = followed.notices.lock().unwrap();
        heard.iter().any(|n| matches!(n, Notice::Stale { .. })).then_some(())
    });
    daemon.restart();
    followed.bootstrapped(2);
    until_some("the window told it reconnected", || {
        let heard = followed.notices.lock().unwrap();
        heard.iter().any(|n| matches!(n, Notice::Reconnected)).then_some(())
    });
    let made = followed.backend.submit(&BackendIntent::CreateTab {
        tab: TabId::new(format!("t{}", NEXT_TAB.fetch_add(1, Ordering::Relaxed))),
        cwd: None,
        run: None,
        name: None,
    });
    assert!(made.is_ok(), "requests work again: {made:?}");
    drop(followed.follower);
}

/// Settings reach a daemon at connect and when they change, which is how the palette and the
/// cursor programs are told come to match the window's, and how a new pane's shell gets the
/// features the config asks for.
#[test]
fn settings_reach_the_daemon() {
    let daemon = Daemon::start_built();
    let followed = follow(&daemon);
    let settings = DaemonSettings {
        scrollback_bytes: Some(1 << 20),
        cursor: Cursor { style: Some(CursorStyle::Bar), blink: Some(false) },
        clipboard_write: ClipboardWrite::Deny,
        shell: Shell {
            mode: ShellMode::Login,
            ssh_env: Some(false),
            sudo: Some(true),
            ..Shell::default()
        },
        ..DaemonSettings::default()
    };
    followed.follower.configure(&settings);

    let mut control = daemon.connect();
    let held = until_some("the settings to arrive", || {
        let settings = snapshot(&mut control).settings.unwrap_or_default();
        (settings.scrollback_bytes == Some(1 << 20)).then_some(settings)
    });
    let cursor = held.cursor.expect("the cursor was sent");
    assert_eq!((cursor.style(), cursor.blink), (proto::CursorStyle::Bar, Some(false)));
    assert_eq!(held.clipboard_write, Some(false), "a program is told it may not copy");
    let shell = held.shell.expect("the shell was sent");
    assert_eq!(shell.mode(), proto::ShellMode::Login);
    assert_eq!((shell.ssh_env, shell.ssh_terminfo, shell.sudo), (Some(false), None, Some(true)));
}

/// A daemon Muster started is in the census with what it holds, asked of it rather than read
/// from the record.
#[test]
fn the_census_asks_each_daemon_what_it_holds() {
    let daemon = Daemon::start_built();
    let followed = follow(&daemon);
    followed
        .backend
        .submit(&BackendIntent::CreateTab {
            tab: TabId::new(format!("t{}", NEXT_TAB.fetch_add(1, Ordering::Relaxed))),
            cwd: None,
            run: None,
            name: None,
        })
        .unwrap();
    let records = daemon.root().join("records");
    let socket = daemon.socket_path().display().to_string();
    records::started(&records.display().to_string(), &socket);

    let census = records::census(&records.display().to_string());
    assert_eq!(census.len(), 1);
    assert_eq!(census[0].state, records::State::Answering);
    assert_eq!(census[0].panes, 1);
}

/// A herdr daemon a Muster from before muster-daemon started is named as one while it listens,
/// and never sent muster-daemon's handshake; once it has gone its record reads like any other.
#[test]
fn a_herdr_daemon_from_before_is_named_in_the_census() {
    let root = std::path::PathBuf::from(format!("/tmp/muster-test/herdr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let socket = root.join("herdr.sock");
    let records = root.join("records").display().to_string();
    let listening = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    records::started(&records, &socket.display().to_string());

    let census = records::census(&records);
    assert_eq!(census[0].state, records::State::Herdr);
    assert_eq!(census[0].panes, 0, "herdr is not asked what it holds");

    // Waited for rather than read once: a child another test forks in this moment holds a copy
    // of the listener until it execs, and the socket accepts on that copy.
    drop(listening);
    until(
        "the record to read silent once nothing listens",
        || records::census(&records)[0].state == records::State::Silent,
        || format!("it reads {:?}", records::census(&records)[0].state),
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A window reacts to what a daemon says by asking it for more - an empty window asks for a
/// tab - and that request's answer is read by the same connection the notice came from. So a
/// reaction asking the daemon something has to be answered, not left waiting on itself.
#[test]
fn a_reaction_to_a_notice_can_ask_the_same_daemon() {
    let daemon = Daemon::start_built();
    let mirror = Arc::new(Mutex::new(Mirror::new()));
    let backend: Arc<Mutex<Option<DaemonBackend>>> = Arc::new(Mutex::new(None));
    let (answered, answers) = std::sync::mpsc::channel();
    let asking = Arc::clone(&backend);
    let follower = Follower::start(
        Following {
            socket: daemon.socket_path().to_path_buf(),
            client: "test".to_string(),
            daemon: "local".to_string(),
            remote: false,
        },
        Arc::clone(&mirror),
        Arc::new(move |notice| {
            if !matches!(notice, Notice::Bootstrapped { .. }) {
                return;
            }
            let backend = until_some("the backend", || asking.lock().unwrap().take());
            let made = backend.submit(&BackendIntent::CreateTab {
                tab: TabId::new("t1"),
                cwd: None,
                run: None,
                name: None,
            });
            let _ = answered.send(made.map(|outcome| outcome.created_tab));
        }),
    )
    .unwrap();
    *backend.lock().unwrap() = Some(DaemonBackend::new(
        follower.connection(),
        Arc::clone(&mirror),
        Arc::new(Mutex::new(Minter::new(Mint::Drawn))),
        BTreeMap::new(),
        "the test daemon".to_string(),
    ));

    let made = answers.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    assert_eq!(made, Ok(Some(TabId::new("t1"))));
}

fn typed(pane: &PaneId, text: &str) -> proto::InputEvent {
    proto::InputEvent {
        pane: pane.to_string(),
        input: Some(proto::input_event::Input::Send(proto::input_event::Send {
            text: text.into(),
            enter: true,
        })),
    }
}

/// A paste larger than the daemon reads in one message is refused here, before it can close
/// the input connection every pane on the daemon shares.
#[test]
fn a_paste_too_large_to_send_leaves_typing_working() {
    let daemon = Daemon::start_built();
    let followed = follow(&daemon);
    let made = followed.backend.submit(&BackendIntent::CreateTab {
        tab: TabId::new(format!("t{}", NEXT_TAB.fetch_add(1, Ordering::Relaxed))),
        cwd: None,
        run: None,
        name: None,
    });
    let pane = made.unwrap().created.unwrap();
    let mut control = daemon.connect();
    until_text(&mut control, pane.as_str(), "$");

    let paste = proto::InputEvent {
        pane: pane.to_string(),
        input: Some(proto::input_event::Input::Paste(proto::input_event::Paste {
            text: "x".repeat(17 << 20),
            confirmed: true,
        })),
    };
    let connection = followed.follower.connection();
    let refused = connection.send_input(paste);
    assert!(matches!(refused, Err(NotSent::TooLarge { .. })), "{refused:?}");
    connection.send_input(typed(&pane, "echo AFTER-THE-PASTE")).unwrap();
    until_text(&mut control, pane.as_str(), "AFTER-THE-PASTE\n");
}

/// Letting go of a daemon that never answers its subscribe hangs the connection up rather than
/// waiting out the subscribe's patience, since whoever lets go may be holding what every other
/// daemon's events need.
#[test]
fn letting_go_of_a_daemon_mid_connect_is_prompt() {
    let daemon = Daemon::start_built();
    let subscribing = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&subscribing);
    let relay = daemon.withholding_answers_where(move |request| {
        let subscribe = matches!(
            &request.service,
            Some(proto::request::Service::Session(proto::SessionRequest {
                request: Some(proto::session_request::Request::Subscribe(_)),
            }))
        );
        seen.fetch_or(subscribe, Ordering::Relaxed);
        subscribe
    });
    let follower = Follower::start(
        Following {
            socket: relay.socket_path().to_path_buf(),
            client: "test".to_string(),
            daemon: "silent".to_string(),
            remote: false,
        },
        Arc::new(Mutex::new(Mirror::new())),
        Arc::new(|_| {}),
    )
    .unwrap();
    until(
        "the follower to be waiting on its subscribe",
        || subscribing.load(Ordering::Relaxed),
        (),
    );

    let started = std::time::Instant::now();
    drop(follower);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "letting go took {:?}",
        started.elapsed()
    );
}
