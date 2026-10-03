//! `muster window new` and `muster window reopen`, asked of the app that is running.
//!
//! Every window of an install is a window of one process (mip/0006-one-process.md), so the command
//! asks the running app for a window and waits for it to be listed. The far end here is a listener
//! standing in for that app, answering with the real schema: it opens a window when asked, and
//! lists it afterwards, which is all the command can see of an app doing so.

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use muster_proto::{
    AskForWindow, Failure, Ok as Accepted, OtherWindow, Request, Response, Window, frame, request,
    response,
};
use prost::Message;

#[test]
fn a_new_window_is_asked_of_the_running_app_and_named_once_open() {
    let scratch = Scratch::new("ask-new");
    let app = App::answering(scratch.home(), 501, "window-1", "window-2");

    let (code, out, errors) = run(&["window", "new"], scratch.home(), BTreeMap::new());

    assert_eq!(code, 0, "asking the running app for a window failed: {errors}");
    assert_eq!(out.trim(), "window-2", "the command did not name the window that opened");
    let asked = app.asked();
    assert_eq!(asked.len(), 1, "the app was not asked exactly once: {asked:?}");
    assert!(asked[0].fresh, "a new window was asked for as a closed one");
    assert_eq!(asked[0].install, muster_daemon_proto::install::INSTALL);
}

#[test]
fn a_closed_window_is_asked_for_by_name() {
    let scratch = Scratch::new("ask-named");
    let app = App::answering(scratch.home(), 502, "window-1", "window-3");

    let (code, out, errors) =
        run(&["window", "reopen", "window-3"], scratch.home(), BTreeMap::new());

    assert_eq!(code, 0, "asking for a closed window by name failed: {errors}");
    assert_eq!(out.trim(), "window-3");
    let asked = app.asked();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(!asked[0].fresh, "a closed window was asked for as a new one");
    assert_eq!(asked[0].name, "window-3");
}

/// An app that will not open a window - another install's, or one from before windows shared a
/// process - is passed over, and the command falls back to starting the app.
#[test]
fn an_app_that_refuses_is_passed_over_for_starting_one() {
    let scratch = Scratch::new("ask-refused");
    let app = App::refusing(scratch.home(), 503, "window-1");
    let environment =
        BTreeMap::from([("MUSTER_APP".to_string(), "/tmp/muster-cli/no-such.app".to_string())]);

    let (code, _, errors) = run(&["window", "new"], scratch.home(), environment);

    assert_ne!(code, 0, "a window was reported opened though nothing could open one");
    assert!(
        errors.contains("no-such.app"),
        "the command did not go on to start the app after the running one refused: {errors}"
    );
    assert_eq!(app.asked().len(), 1, "the running app was not asked first");
}

/// What a stand-in app was asked, and whether it says yes.
struct App {
    asked: Arc<Mutex<Vec<AskForWindow>>>,
}

impl App {
    /// Opens `opens` when asked, and lists it from then on beside `name`.
    fn answering(home: &Path, pid: u32, name: &str, opens: &str) -> App {
        App::listening(home, pid, name, Some(opens.to_string()))
    }

    fn refusing(home: &Path, pid: u32, name: &str) -> App {
        App::listening(home, pid, name, None)
    }

    fn listening(home: &Path, pid: u32, name: &str, opens: Option<String>) -> App {
        let path = home.join("state").join(format!("command-{pid}.sock"));
        let listener = UnixListener::bind(&path).expect("the temporary directory is writable");
        let asked = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&asked);
        let name = name.to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let Ok(bytes) = frame::read_frame(&mut stream, frame::LARGEST_MESSAGE) else {
                    continue;
                };
                let request = Request::decode(bytes.as_slice()).unwrap_or_default();
                let answer = if let Some(request::Payload::AskForWindow(ask)) = request.payload {
                    heard.lock().expect("a test thread panicked").push(ask);
                    if opens.is_some() {
                        response::Payload::Ok(Accepted {})
                    } else {
                        response::Payload::Failure(Failure {
                            reason: "this app belongs to another install".to_string(),
                        })
                    }
                } else {
                    let opened = !heard.lock().expect("a test thread panicked").is_empty();
                    let others = opens
                        .iter()
                        .filter(|_| opened)
                        .map(|window| OtherWindow {
                            name: window.clone(),
                            pid,
                            ..OtherWindow::default()
                        })
                        .collect();
                    response::Payload::Window(Window {
                        name: name.clone(),
                        windows: others,
                        ..Window::default()
                    })
                };
                let encoded = Response { payload: Some(answer) }.encode_to_vec();
                let _ = frame::write_frame(&mut stream, &encoded);
                let mut drained = Vec::new();
                let _ = stream.read_to_end(&mut drained);
            }
        });
        App { asked }
    }

    fn asked(&self) -> Vec<AskForWindow> {
        self.asked.lock().expect("a test thread panicked").clone()
    }
}

/// A home of its own per test, under `/tmp` so a socket path fits in the hundred bytes it has.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(named: &str) -> Scratch {
        let root = PathBuf::from("/tmp/muster-cli").join(named);
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

fn run(
    argv: &[&str],
    home: &Path,
    mut environment: BTreeMap<String, String>,
) -> (i32, String, String) {
    environment.insert("MUSTER_HOME".to_string(), home.to_string_lossy().into_owned());
    let argv: Vec<String> = argv.iter().map(|word| (*word).to_string()).collect();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let code =
        muster_cli::run(&argv, &environment, None, &mut std::io::empty(), &mut out, &mut errors);
    (
        code,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&errors).into_owned(),
    )
}
