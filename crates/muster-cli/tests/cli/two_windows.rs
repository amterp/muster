//! What `muster` does when nobody said which Muster, and more than one is listening.
//!
//! Every window of an app answers on one socket, so two sockets answering are two apps: two
//! installs under one home, such as a development build beside the release. And `muster window
//! list` turns one app's answer into a row per window.
//!
//! No daemon and no app here, deliberately. The decision under test is the CLI's own and is made
//! before anything is dialled - is this a question, and did the caller name an app - and the only
//! thing it needs from the other end is that something answers on two sockets.
//!
//! So the far end here is two listeners answering a canned `Window`. That is not a stand-in for a
//! daemon, which this repo does not have: it is a stand-in for a peer *client of this CLI's own
//! protocol*, and the protobuf and the framing it answers with are the real ones from
//! `muster-proto`.

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use muster_cli::dial;
use muster_proto::{
    Daemons, Failure, KnownDaemon, Names, Ok as Accepted, OtherWindow, PaneText, Request, Response,
    RosterChanged, RosterPane, RosterTab, Window, frame, request, response,
};
use prost::Message;

#[test]
fn a_question_nobody_narrowed_is_answered_by_every_app() {
    let scratch = Scratch::new("every");
    let home = scratch.home();
    let first = window(home, 111, "first-pane");
    let second = window(home, 222, "second-pane");

    let (code, out, errors) = run(&["window"], home, None);

    assert_eq!(code, 0, "muster window refused with two apps open: {errors}");
    for expected in [&first, &second, &first_socket(home, 111), &first_socket(home, 222)] {
        assert!(
            out.contains(expected.as_str()),
            "the answer does not mention {expected}, so one of the two apps is missing from \
             it:\n{out}"
        );
    }
}

#[test]
fn one_window_answers_exactly_as_it_did_before() {
    let scratch = Scratch::new("one");
    let home = scratch.home();
    let only = window(home, 333, "only-pane");

    let (code, out, _) = run(&["window"], home, None);

    assert_eq!(code, 0);
    assert!(out.contains(&only), "the one window's answer is missing its pane:\n{out}");
    // No heading, because there is nothing to tell apart. A script reading one window's output
    // is the case that must not move, and this is the shape of that promise.
    assert!(
        !out.contains(&first_socket(home, 333)),
        "one app's answer grew a heading, so every caller reading it has to learn about apps in \
         the plural:\n{out}"
    );
}

#[test]
fn a_caller_inside_a_pane_still_hears_only_its_own_window() {
    let scratch = Scratch::new("pane");
    let home = scratch.home();
    let first = window(home, 444, "first-pane");
    let second = window(home, 555, "second-pane");

    let (code, out, _) = run(&["window"], home, Some(&first_socket(home, 444)));

    assert_eq!(code, 0);
    assert!(out.contains(&first), "the pane's own window is not in the answer:\n{out}");
    assert!(
        !out.contains(&second),
        "a command run inside a pane answered about another window too, which is what \
         $MUSTER_SOCKET exists to prevent:\n{out}"
    );
}

/// Muster relaunched: the window that made this pane has quit, and a new one of the same install
/// holds its tab now. The pane's `$MUSTER_SOCKET` still names the old window, and what it runs
/// reaches the new one, which carries a change to whichever window holds the pane.
#[test]
fn a_pane_whose_window_quit_reaches_the_window_open_now() {
    let scratch = Scratch::new("relaunched");
    let home = scratch.home();
    let gone = quit_window(home, 111);
    let live = window(home, 222, "live-pane");

    let (code, out, errors) = run_in_pane(&["window"], home, &gone, "old-pane");
    assert_eq!(code, 0, "the pane could not ask the window open now: {errors}");
    assert!(out.contains(&live), "the answer is not the open window's:\n{out}");

    let (code, _, errors) = run_in_pane(&["pane", "new", "--down"], home, &gone, "old-pane");
    assert_ne!(code, 3, "a change from the pane found no window: {errors}");
    assert!(!errors.contains("has quit"), "the pane was told its window has quit:\n{errors}");
}

/// Two windows of the install are open and the pane's own has quit. Its change names its pane, so
/// either window can take it and carry it to the one holding the pane; a question is answered by
/// both, as it is for a caller outside every pane.
#[test]
fn with_two_windows_open_a_pane_whose_window_quit_reaches_them() {
    let scratch = Scratch::new("relaunched-two");
    let home = scratch.home();
    let gone = quit_window(home, 111);
    let first = window(home, 333, "first-pane");
    let second = window(home, 444, "second-pane");

    let (code, _, errors) = run_in_pane(&["pane", "new", "--down"], home, &gone, "old-pane");
    assert_ne!(code, 3, "a change naming its pane was refused for want of a window: {errors}");
    assert!(!errors.contains("--socket"), "the pane was asked which window it meant:\n{errors}");

    let (code, out, errors) = run_in_pane(&["window"], home, &gone, "old-pane");
    assert_eq!(code, 0, "{errors}");
    for expected in [&first, &second] {
        assert!(out.contains(expected), "{expected} is missing from the answer:\n{out}");
    }
}

#[test]
fn a_change_with_two_windows_open_still_refuses_and_names_them() {
    let scratch = Scratch::new("write");
    let home = scratch.home();
    window(home, 666, "first-pane");
    window(home, 777, "second-pane");

    let (code, out, errors) = run(&["pane", "new", "--down"], home, None);

    assert_eq!(code, 3, "a change was carried out with nothing saying which window it was for");
    assert!(out.is_empty(), "a refused command wrote to stdout: {out}");
    for expected in ["command-666.sock", "command-777.sock", "--socket"] {
        assert!(
            errors.contains(expected),
            "the refusal does not mention {expected}, so it does not say how to pick one:\n\
             {errors}"
        );
    }
}

/// A change that names its tab or pane goes to any window, because the window it reaches carries
/// it to the one holding that tab (kan a_2Mhi0EZlv): it does not matter which window a command
/// is run from. One naming nothing still has to say which, above.
#[test]
fn a_change_naming_its_tab_goes_to_a_window_with_two_open() {
    let scratch = Scratch::new("named");
    let home = scratch.home();
    window(home, 611, "first-pane");
    window(home, 622, "second-pane");

    let (code, _, errors) = run(&["tab", "focus", "t1w3r07bsd"], home, None);

    assert_ne!(code, 3, "a change naming its tab was refused for want of a window: {errors}");
    assert!(
        !errors.contains("--socket"),
        "a change naming its tab asked which window it was for:\n{errors}"
    );
}

/// A tab move that names the tab and where it goes goes to any window too: it is a write to the
/// record every window shares, so any window can make it. One naming no window means "here", and
/// still has to say which window that is.
#[test]
fn a_tab_move_naming_where_it_goes_reaches_a_window_with_two_open() {
    let scratch = Scratch::new("move");
    let home = scratch.home();
    window(home, 633, "first-pane");
    window(home, 644, "second-pane");

    let (code, _, errors) =
        run(&["tab", "move", "--tab", "t1w3r07bsd", "--window", "window-2"], home, None);
    assert_ne!(code, 3, "a tab move naming its window was refused for want of one: {errors}");
    assert!(
        !errors.contains("--socket"),
        "a tab move naming where it goes asked which window it was for:\n{errors}"
    );

    let (code, _, errors) = run(&["tab", "move", "--tab", "t1w3r07bsd"], home, None);
    assert_eq!(code, 3, "a tab move to \"here\" went somewhere with two windows open: {errors}");
    assert!(errors.contains("--socket"), "the refusal does not say how to pick one:\n{errors}");
}

#[test]
fn a_program_reading_two_apps_gets_one_object_per_app() {
    let scratch = Scratch::new("json");
    let home = scratch.home();
    window(home, 888, "first-pane");
    window(home, 999, "second-pane");

    let (code, out, _) = run(&["window", "--json"], home, None);

    assert_eq!(code, 0);
    let answered: serde_json::Value =
        serde_json::from_str(&out).unwrap_or_else(|error| panic!("not JSON ({error}): {out}"));
    let windows = answered["windows"].as_array().expect("an object with a windows array");
    assert_eq!(windows.len(), 2, "{out}");
    // Flattened rather than nested, so a filter written against one window's answer reads across
    // several: `.windows[].panes[] | select(...)`.
    for window in windows {
        assert!(window["panes"].is_array(), "a window's row carries no panes: {window}");
        assert!(window["socket"].is_string(), "an app's row does not say which socket: {window}");
    }
}

/// The other windows of an app are listed under their names, and a closed one says so.
///
/// A closed window keeps its tabs and its agents keep running (kan a_2Mhi0EZlv), and the app is
/// what answers for them - so the window that answered lists every other window of its app.
#[test]
fn the_other_windows_are_listed_under_their_names() {
    let scratch = Scratch::new("closed");
    let home = scratch.home();
    answering(home, 631, "window-1", vec![closed_window(), open_window()]);

    let (code, out, errors) = run(&["window"], home, None);
    assert_eq!(code, 0, "{errors}");
    assert!(out.contains("window-3 (closed)"), "the closed window is not named:\n{out}");
    assert!(out.contains("left running"), "the closed window's tab is not listed:\n{out}");
    assert!(
        out.contains("window-2") && !out.contains("632"),
        "an open window is not headed by the name `--window` takes, and only that:\n{out}"
    );
}

/// `window list` is a row per window, not per app: the one that answered and the other open
/// ones, and with `--closed` the closed ones instead.
#[test]
fn window_list_lists_windows_and_closed_ones_on_their_own() {
    let scratch = Scratch::new("list");
    let home = scratch.home();
    answering(home, 641, "window-1", vec![closed_window(), open_window()]);

    let (code, out, errors) = run(&["window", "list", "--json"], home, None);
    assert_eq!(code, 0, "{errors}");
    let listed: serde_json::Value =
        serde_json::from_str(&out).unwrap_or_else(|error| panic!("not JSON ({error}): {out}"));
    let names: Vec<&str> = listed["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["window"].as_str())
        .collect();
    assert_eq!(names, vec!["window-1", "window-2"], "{out}");

    let (code, out, errors) = run(&["window", "list", "--closed"], home, None);
    assert_eq!(code, 0, "{errors}");
    assert!(out.contains("window-3"), "the closed window is not listed:\n{out}");
    assert!(
        !out.contains("window-1") && !out.contains("window-2"),
        "--closed listed an open window:\n{out}"
    );
}

fn closed_window() -> OtherWindow {
    OtherWindow {
        name: "window-3".to_string(),
        pid: 0,
        tabs: vec![RosterTab {
            tab_id: "t3closed".to_string(),
            label: "left running".to_string(),
            ..RosterTab::default()
        }],
    }
}

fn open_window() -> OtherWindow {
    OtherWindow { name: "window-2".to_string(), pid: 632, tabs: Vec::new() }
}

/// A pane's text read with two apps listening is the text, from the app holding the pane, rather
/// than a heading per app and a word for what each answered.
#[test]
fn a_pane_read_with_two_apps_open_is_the_text_from_the_one_holding_it() {
    let scratch = Scratch::new("read-two");
    let home = scratch.home();
    reading(home, 111, "p-here");
    reading(home, 222, "p-elsewhere");

    let (code, out, errors) = run(&["pane", "read", "--pane", "p-here"], home, None);

    assert_eq!(code, 0, "reading a pane with two apps open failed: {errors}");
    assert_eq!(out.trim(), "the text of p-here", "the read did not print the pane's text: {out}");
}

/// The daemons each app follows, asked of two apps, are each app's answer under its heading.
#[test]
fn the_daemons_asked_of_two_apps_are_each_apps_answer() {
    let scratch = Scratch::new("daemons-two");
    let home = scratch.home();
    reading(home, 111, "p-here");
    reading(home, 222, "p-elsewhere");

    let (code, out, errors) = run(&["daemons"], home, None);

    assert_eq!(code, 0, "{errors}");
    for socket in ["/s/daemon-111.sock", "/s/daemon-222.sock"] {
        assert!(out.contains(socket), "an app's daemons are missing:\n{out}");
    }
    assert!(!out.contains("a list of daemons"), "an app's answer was named, not shown:\n{out}");
}

/// Two apps answer, and only one holds the pane a command is about: the command goes there, and
/// the other is asked nothing but what it is showing. This is a pane whose window quit with two
/// apps open beside it, which used to refuse a wait and send a change to whichever app answered
/// first (kan a_2cW584sro).
#[test]
fn a_pane_whose_window_quit_reaches_the_app_holding_it() {
    let scratch = Scratch::new("holder");
    let home = scratch.home();
    let gone = quit_window(home, 111);
    let elsewhere = holding(&first_socket(home, 333), "p-elsewhere");
    let holder = holding(&first_socket(home, 444), "p-here");

    for argv in [
        &["pane", "send", "--pane", "p-here", "hi"][..],
        &["pane", "wait", "--pane", "p-here", "--until", "idle", "--timeout", "5"],
        &["pane", "close"],
    ] {
        let (code, _, errors) = run_in_pane(argv, home, &gone, "p-here");
        assert_eq!(
            code,
            0,
            "`{}` did not reach the app holding the pane: {errors}",
            argv.join(" ")
        );
    }
    assert_eq!(asked(&holder).len(), 3, "the holder was not sent all three");
    assert_eq!(asked(&elsewhere), Vec::<Request>::new(), "the other app was sent a command");
}

/// A pane with no `$MUSTER_SOCKET` - restored after its daemon restarted, or run under something
/// that cleared it - still says which pane it is, and reaches the app holding it.
#[test]
fn a_pane_with_no_socket_reaches_the_app_holding_it() {
    let scratch = Scratch::new("unset");
    let home = scratch.home();
    let elsewhere = holding(&first_socket(home, 333), "p-elsewhere");
    let holder = holding(&first_socket(home, 444), "p-here");

    let environment = BTreeMap::from([("MUSTER_PANE".to_string(), "p-here".to_string())]);
    let (code, _, errors) = run_with(&["pane", "close"], home, environment);

    assert_eq!(code, 0, "{errors}");
    assert_eq!(asked(&holder).len(), 1, "the holder was not asked to close the pane");
    assert!(asked(&elsewhere).is_empty(), "the other app was sent the close");
}

/// On a devenv the windows are forwarded beside the daemon's socket, not kept in the state
/// directory, so a pane with no `$MUSTER_SOCKET` looks there: the measured case was five sockets
/// beside the daemon, four of them dead and one holding the pane.
#[test]
fn a_pane_with_no_socket_finds_the_windows_forwarded_beside_its_daemon() {
    let scratch = Scratch::new("devenv");
    let home = scratch.home();
    let beside = home.join("daemon");
    std::fs::create_dir_all(&beside).expect("/tmp is writable");
    let socket = |window: &str| {
        beside.join(format!("window-release-{window}.sock")).to_string_lossy().into_owned()
    };
    quit_window_at(&socket("dead"));
    let elsewhere = holding(&socket("other"), "p-elsewhere");
    let holder = holding(&socket("mine"), "p-here");

    let environment = BTreeMap::from([
        ("MUSTER_PANE".to_string(), "p-here".to_string()),
        (
            "MUSTER_DAEMON_SOCKET".to_string(),
            beside.join("release.sock").to_string_lossy().into_owned(),
        ),
    ]);
    let (code, _, errors) =
        run_with(&["pane", "send", "--pane", "p-here", "hi"], home, environment);

    assert_eq!(code, 0, "the pane did not find the window forwarded beside its daemon: {errors}");
    assert_eq!(asked(&holder).len(), 1);
    assert!(asked(&elsewhere).is_empty(), "the other forwarded window was sent the send");
}

/// A pane that says which pane it is is not told it is not running in a pane Muster made.
#[test]
fn a_pane_with_no_socket_and_no_window_is_told_what_was_searched() {
    let scratch = Scratch::new("searched");
    let home = scratch.home();

    let environment = BTreeMap::from([("MUSTER_PANE".to_string(), "p-here".to_string())]);
    let (code, _, errors) = run_with(&["pane", "new", "--down"], home, environment);

    assert_eq!(code, 3, "{errors}");
    assert!(
        !errors.contains("not running in a pane Muster made"),
        "a pane that named itself was told it is not in one:\n{errors}"
    );
    assert!(errors.contains("not set in this pane (p-here)"), "{errors}");
}

/// What a [`holding`] app was sent besides the question every request starts with.
fn asked(sent: &Receiver<Request>) -> Vec<Request> {
    sent.try_iter().collect()
}

/// An app holding one pane: what it is showing says so, a request about another pane is refused,
/// and anything else is done. Hands back everything it was sent but the question what it is
/// showing.
fn holding(path: &str, pane: &str) -> Receiver<Request> {
    let listener = UnixListener::bind(path).expect("the temporary directory is writable");
    let pane = pane.to_string();
    let (heard, sent) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Ok(bytes) = frame::read_frame(&mut stream, frame::LARGEST_MESSAGE) else {
                continue;
            };
            let asked = Request::decode(bytes.as_slice()).unwrap_or_default();
            let answer = match (&asked.payload, dial::named(&asked)) {
                (Some(request::Payload::ReadWindow(_)), _) => {
                    response::Payload::Window(holding_window(&pane))
                }
                (_, Some(Names::Pane(named))) if named != pane => {
                    response::Payload::Failure(Failure { reason: format!("no pane {named}") })
                }
                _ => response::Payload::Ok(Accepted {}),
            };
            if !matches!(asked.payload, Some(request::Payload::ReadWindow(_))) {
                let _ = heard.send(asked);
            }
            let _ = frame::write_frame(
                &mut stream,
                &Response { payload: Some(answer) }.encode_to_vec(),
            );
            let mut drained = Vec::new();
            let _ = stream.read_to_end(&mut drained);
        }
    });
    sent
}

/// A window whose one tab holds `pane`.
fn holding_window(pane: &str) -> Window {
    Window {
        roster: Some(RosterChanged {
            tabs: vec![RosterTab {
                tab_id: format!("t-{pane}"),
                panes: vec![RosterPane { pane_id: pane.to_string(), ..RosterPane::default() }],
                ..RosterTab::default()
            }],
            ..RosterChanged::default()
        }),
        ..Window::default()
    }
}

/// An app answering reads: a pane's text for the one pane it holds, refusing every other, and
/// its daemons.
fn reading(home: &Path, pid: u32, holds: &str) {
    let listener =
        UnixListener::bind(first_socket(home, pid)).expect("the temporary directory is writable");
    let holds = holds.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Ok(bytes) = frame::read_frame(&mut stream, frame::LARGEST_MESSAGE) else {
                continue;
            };
            let asked = Request::decode(bytes.as_slice()).unwrap_or_default();
            let answer = match asked.payload {
                Some(request::Payload::ReadPane(read)) if read.pane_id == holds => {
                    response::Payload::PaneText(PaneText {
                        text: format!("the text of {holds}"),
                        ..PaneText::default()
                    })
                }
                Some(request::Payload::ReadDaemons(_)) => response::Payload::Daemons(Daemons {
                    remembered: true,
                    daemons: vec![KnownDaemon {
                        socket: format!("/s/daemon-{pid}.sock"),
                        state: "answering".to_string(),
                        ..KnownDaemon::default()
                    }],
                }),
                _ => response::Payload::Failure(Failure {
                    reason: "no window here holds that pane".to_string(),
                }),
            };
            let _ = frame::write_frame(
                &mut stream,
                &Response { payload: Some(answer) }.encode_to_vec(),
            );
            let mut drained = Vec::new();
            let _ = stream.read_to_end(&mut drained);
        }
    });
}

/// A listener answering as a window with other windows beside it.
fn answering(home: &Path, pid: u32, name: &str, others: Vec<OtherWindow>) {
    let path = first_socket(home, pid);
    let listener = UnixListener::bind(&path).expect("the temporary directory is writable");
    let answer = Response {
        payload: Some(response::Payload::Window(Window {
            name: name.to_string(),
            windows: others,
            ..Window::default()
        })),
    }
    .encode_to_vec();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = frame::read_frame(&mut stream, frame::LARGEST_MESSAGE);
            let _ = frame::write_frame(&mut stream, &answer);
            let mut drained = Vec::new();
            let _ = stream.read_to_end(&mut drained);
        }
    });
}

/// A home of its own per test, since the CLI finds windows by reading one directory, and per
/// process: two checkouts running the suite at once would otherwise delete each other's sockets.
///
/// Under `/tmp` rather than under the platform's temporary directory, and named as briefly as
/// this reads: a unix socket path has about a hundred bytes to spend, and macOS hands out
/// `/var/folders/<two>/<long>/T/` which spends most of them before a name is written. The same
/// reason `muster-harness` picks its own root.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(named: &str) -> Scratch {
        let root = PathBuf::from("/tmp/muster-cli").join(format!("{}-{named}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("state")).expect("/tmp is writable");
        Scratch { root }
    }

    fn home(&self) -> &Path {
        &self.root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn first_socket(home: &Path, pid: u32) -> String {
    home.join("state").join(format!("command-{pid}.sock")).to_string_lossy().into_owned()
}

/// A listener answering as a window would, for as long as the test runs.
///
/// Answers every connection rather than one: `survey` dials each socket once per command, and a
/// test that ran two commands against one listener would find the second unanswered.
fn window(home: &Path, pid: u32, pane: &str) -> String {
    let path = first_socket(home, pid);
    let listener = UnixListener::bind(&path).expect("the temporary directory is writable");
    let answer = Response {
        payload: Some(response::Payload::Window(Window {
            roster: Some(RosterChanged {
                tabs: vec![RosterTab {
                    daemon_ids: vec!["local".to_string()],
                    tab_id: format!("t{pid}"),
                    place: 1,
                    label: format!("tab of {pid}"),
                    panes: vec![RosterPane {
                        daemon_id: "local".to_string(),
                        pane_id: pane.to_string(),
                        place: 1,
                        label: pane.to_string(),
                        on_screen: true,
                        ..RosterPane::default()
                    }],
                    ..RosterTab::default()
                }],
                ..RosterChanged::default()
            }),
            ..Window::default()
        })),
    }
    .encode_to_vec();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // Read the request off the socket before answering, because the other end writes and
            // then reads: answering without draining would leave a frame in the buffer and the
            // next command talking to a socket that is out of step.
            let _ = frame::read_frame(&mut stream, frame::LARGEST_MESSAGE);
            let _ = frame::write_frame(&mut stream, &answer);
            let mut drained = Vec::new();
            let _ = stream.read_to_end(&mut drained);
        }
    });
    pane.to_string()
}

/// Where a window that has quit listened: its socket file left behind, with nothing answering.
fn quit_window(home: &Path, pid: u32) -> String {
    let path = first_socket(home, pid);
    quit_window_at(&path);
    path
}

fn quit_window_at(path: &str) {
    drop(UnixListener::bind(path).expect("the temporary directory is writable"));
}

fn run(argv: &[&str], home: &Path, in_a_pane: Option<&str>) -> (i32, String, String) {
    let mut environment = BTreeMap::new();
    if let Some(socket) = in_a_pane {
        environment.insert("MUSTER_SOCKET".to_string(), socket.to_string());
    }
    run_with(argv, home, environment)
}

/// A command run inside `pane`, whose window listened at `socket`.
fn run_in_pane(argv: &[&str], home: &Path, socket: &str, pane: &str) -> (i32, String, String) {
    let environment = BTreeMap::from([
        ("MUSTER_SOCKET".to_string(), socket.to_string()),
        ("MUSTER_PANE".to_string(), pane.to_string()),
    ]);
    run_with(argv, home, environment)
}

fn run_with(
    argv: &[&str],
    home: &Path,
    mut environment: BTreeMap<String, String>,
) -> (i32, String, String) {
    environment.insert("MUSTER_HOME".to_string(), home.to_string_lossy().into_owned());
    let argv: Vec<String> = argv.iter().map(|word| (*word).to_string()).collect();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    // No directory: nothing here names one, and which window a command is about is decided
    // before any path is read.
    let code =
        muster_cli::run(&argv, &environment, None, &mut std::io::empty(), &mut out, &mut errors);
    (
        code,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&errors).into_owned(),
    )
}
