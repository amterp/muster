//! A request about another window's tab is carried to that window (kan a_2Mhi0EZlv).
//!
//! Alex's rule: any `muster` verb works from any window. A tab belongs to exactly one window, so
//! the window a caller reached hands a request about another window's tab to that window, over
//! its command socket, and relays the answer. These drive this window's own command socket the
//! way the CLI does, with a stand-in for the other window that records what it was carried.

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use herdr_harness::{Daemon, until};
use muster::proto::{
    Carried, CloseTab, CreateTab, Event, FocusTab, OpenWindow, ReadTabHolders, ReadWindow,
    RenameTab, Request, Response, Startup, event, request, response,
};
use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::composition::{HeldWindow, Holders, WindowName};
use muster_core::mirror::backend::TabId;
use muster_proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use prost::Message;
use serde_json::json;

/// Going to, renaming and closing a tab another window holds all reach that window.
#[test]
fn a_request_about_another_windows_tab_is_carried_there() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let other = Stand::in_for(&daemon, "window-9");
    let ours = open_a_window(&daemon, "window-1");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-9");

    for request in [
        request::Payload::FocusTab(FocusTab { tab_id: theirs.clone(), ..FocusTab::default() }),
        request::Payload::RenameTab(RenameTab {
            tab_id: theirs.clone(),
            name: "renamed".to_string(),
            ..RenameTab::default()
        }),
        request::Payload::CloseTab(CloseTab { tab_id: theirs.clone(), ..CloseTab::default() }),
    ] {
        let answer = ask(&ours, request.clone());
        assert!(
            matches!(answer.payload, Some(response::Payload::Ok(_))),
            "the other window's answer was not relayed: {answer:?}"
        );
        let carried = other.last().expect("the other window was carried nothing");
        assert_eq!(carried.by, "window-1", "the carried request does not say who carried it");
        assert_eq!(
            carried.request.and_then(|request| request.payload),
            Some(request),
            "the other window was carried something other than what was asked"
        );
    }
    // Carried and not also done here: the stand-in closes nothing, so a tab gone from the daemon
    // would be this window acting on another window's tab after all.
    assert_eq!(daemon_tabs(&daemon).len(), 2, "the close was carried out here as well as carried");
}

/// A request carried to this window is answered here, and never carried on.
///
/// Two windows whose records briefly disagree would otherwise hand a request back and forth. And
/// going to a tab this way brings the window forward, because whoever asked was looking at
/// something else.
#[test]
fn a_request_carried_here_is_answered_here() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let other = Stand::in_for(&daemon, "window-9");
    let ours = open_a_window(&daemon, "window-1");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-9");
    RAISED.lock().expect("a panicking test poisoned the flag").take();

    // Carried here although the record says the other window has it: this window answers, and
    // refuses rather than showing another window's tab.
    let answer = ask(
        &ours,
        request::Payload::Carried(Box::new(Carried {
            by: "window-9".to_string(),
            request: Some(Box::new(Request {
                payload: Some(request::Payload::FocusTab(FocusTab {
                    tab_id: theirs,
                    ..FocusTab::default()
                })),
            })),
        })),
    );
    assert!(other.last().is_none(), "a carried request was carried on");
    assert!(
        matches!(answer.payload, Some(response::Payload::Failure(_))),
        "this window went to a tab it does not hold: {answer:?}"
    );

    // And one of its own tabs, carried here, is gone to and brings the window forward.
    let own = listed().first().cloned().expect("the window holds a tab");
    let answer = ask(
        &ours,
        request::Payload::Carried(Box::new(Carried {
            by: "window-9".to_string(),
            request: Some(Box::new(Request {
                payload: Some(request::Payload::FocusTab(FocusTab {
                    tab_id: own,
                    ..FocusTab::default()
                })),
            })),
        })),
    );
    assert!(matches!(answer.payload, Some(response::Payload::Ok(_))), "{answer:?}");
    assert!(
        RAISED.lock().expect("a panicking test poisoned the flag").is_some(),
        "going to a tab from another window left this window behind whatever was in front"
    );
}

/// A tab whose window is closed is closed from whichever window was asked.
///
/// The daemon does the closing, and the closed window only remembers the tab. Refusing would
/// leave a tab nothing could close until its window was reopened.
#[test]
fn a_closed_windows_tab_is_closed_from_here() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let ours = open_a_window(&daemon, "window-1");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-8");
    let before = daemon_tabs(&daemon).len();

    let answer =
        ask(&ours, request::Payload::CloseTab(CloseTab { tab_id: theirs, ..CloseTab::default() }));
    assert!(matches!(answer.payload, Some(response::Payload::Ok(_))), "{answer:?}");
    until(
        "the daemon to close the closed window's tab",
        || daemon_tabs(&daemon).len() == before - 1,
        || format!("the daemon still holds {:?}", daemon_tabs(&daemon)),
    );
}

/// The other window, as far as this one can tell: a socket that answers, named in the record.
struct Stand {
    carried: Arc<Mutex<Vec<Carried>>>,
}

impl Stand {
    fn in_for(daemon: &Daemon, name: &str) -> Stand {
        let socket = daemon.root().join(format!("{name}.sock"));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).expect("a socket can be bound");
        let arrangement = daemon.root().join(format!("{name}.toml"));
        std::fs::write(&arrangement, "").expect("a stand-in arrangement can be written");
        let path = record(daemon);
        let mut holders = read_record(&path);
        holders.opened(HeldWindow {
            name: WindowName::new(name),
            arrangement: arrangement.to_string_lossy().into_owned(),
            socket: socket.to_string_lossy().into_owned(),
            pid: 1,
            focused: 0,
        });
        write_record(&path, &holders);

        let carried = Arc::new(Mutex::new(Vec::new()));
        let noted = Arc::clone(&carried);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                // A window asking whether this one is open connects and says nothing.
                let Ok(bytes) = read_frame(&mut stream, LARGEST_MESSAGE) else { continue };
                if let Ok(Request { payload: Some(request::Payload::Carried(carried)) }) =
                    Request::decode(bytes.as_slice())
                {
                    noted.lock().expect("a panicking test poisoned the log").push(*carried);
                }
                let _ = write_frame(&mut stream, &Response::ok().encode_to_vec());
            }
        });
        Stand { carried }
    }

    fn last(&self) -> Option<Carried> {
        self.carried.lock().expect("a panicking test poisoned the log").pop()
    }
}

/// Makes a second tab in this window and gives it to another one, as that window taking it would.
fn a_second_tab_given_to(daemon: &Daemon, ours: &Path, window: &str) -> String {
    assert!(matches!(
        ask(
            ours,
            request::Payload::CreateTab(CreateTab { take_focus: true, ..CreateTab::default() })
        )
        .payload,
        Some(response::Payload::Made(_) | response::Payload::Ok(_))
    ));
    until(
        "the second tab to arrive",
        || listed().len() == 2,
        || format!("this window lists {:?}", listed()),
    );
    let theirs = listed().last().cloned().expect("just waited for it");
    let path = record(daemon);
    let mut holders = read_record(&path);
    holders.take(TabId::new(&theirs), &WindowName::new(window));
    write_record(&path, &holders);
    assert_ok(&answer(request::Payload::ReadTabHolders(ReadTabHolders {})));
    assert_eq!(listed().len(), 1, "the tab given away is still listed here");
    theirs
}

fn open_a_window(daemon: &Daemon, name: &str) -> PathBuf {
    muster::ffi::muster_set_event_callback(Some(note));
    let socket = daemon.root().join(format!("{name}.sock"));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: daemon.root().join(format!("{name}.toml")).to_string_lossy().into_owned(),
        pane_names_path: daemon.root().join("panes.toml").to_string_lossy().into_owned(),
        tab_holders_path: record(daemon).to_string_lossy().into_owned(),
        command_socket_path: socket.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the window to open onto a tab",
        || !listed().is_empty(),
        || "the window lists no tab".to_string(),
    );
    socket
}

/// Asks this window over its command socket, the way the CLI does.
fn ask(socket: &Path, payload: request::Payload) -> Response {
    let mut stream = UnixStream::connect(socket).expect("the window is listening");
    write_frame(&mut stream, &Request { payload: Some(payload) }.encode_to_vec())
        .expect("the request can be written");
    let bytes = read_frame(&mut stream, LARGEST_MESSAGE).expect("the window answers");
    Response::decode(bytes.as_slice()).expect("the window answers with a response")
}

fn daemon_tabs(daemon: &Daemon) -> Vec<String> {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    snapshot["snapshot"]["tabs"]
        .as_array()
        .map(|tabs| {
            tabs.iter().filter_map(|tab| tab["tab_id"].as_str().map(str::to_string)).collect()
        })
        .unwrap_or_default()
}

fn record(daemon: &Daemon) -> PathBuf {
    daemon.root().join("holding/tabs.toml")
}

fn read_record(path: &Path) -> Holders {
    from_toml(&std::fs::read_to_string(path).unwrap_or_default())
        .expect("the record this window writes reads back")
}

fn write_record(path: &Path, holders: &Holders) {
    std::fs::create_dir_all(path.parent().expect("the record is in a directory"))
        .expect("the record's directory can be made");
    std::fs::write(path, to_toml(holders)).expect("the record can be written");
}

fn listed() -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window
            .roster
            .iter()
            .flat_map(|roster| roster.tabs.iter())
            .map(|tab| tab.tab_id.clone())
            .collect(),
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

static RAISED: Mutex<Option<()>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::RaiseWindow(_)) = event.payload {
        *RAISED.lock().expect("a panicking test poisoned the flag") = Some(());
    }
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
