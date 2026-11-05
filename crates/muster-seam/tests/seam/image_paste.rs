use muster::proto::{FocusPane, PasteImage, PrepareImagePaste, SendToPane, request, response};
use muster_core::input::image::quoted_image_path;
use muster_daemon_proto::connection;
use muster_daemon_proto::pane_request;
use muster_harness::requests::{create, in_new_tab, make, until_text};
use muster_harness::{Daemon, until};
use muster_harness::{Pump, Relay};
use prost::Message;
use std::io;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};

use super::surface_input::{answer, assert_ok, open_at, open_on, read};

static IMAGE_FINISHED: Mutex<bool> = Mutex::new(false);

extern "C" fn image_status(bytes: *const u8, len: usize) {
    // SAFETY: the callback owns a readable buffer for this call, as include/muster.h defines.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = muster::proto::Event::decode(bytes).unwrap();
    if let Some(muster::proto::event::Payload::ImagePasteChanged(changed)) = event.payload
        && !changed.pasting
        && changed.pane_id == "image-restart-shell"
    {
        *IMAGE_FINISHED.lock().unwrap() = true;
    }
}

fn prepare() -> u64 {
    match answer(request::Payload::PrepareImagePaste(PrepareImagePaste::default())).payload {
        Some(response::Payload::ImagePastePrepared(prepared)) => prepared.upload_id,
        other => panic!("preparing the image answered {other:?}"),
    }
}

fn capture(daemon: &Daemon, pane: &str) -> std::path::PathBuf {
    let path = daemon.root().join(format!("{pane}.input"));
    make(
        &mut daemon.connect(),
        pane_request::Create {
            command: Some(format!("stty raw -echo; echo READY; cat > {}", path.display())),
            ..create(pane, in_new_tab(pane))
        },
    );
    until_text(&mut daemon.connect(), pane, "READY");
    path
}

fn send(pane: &str, text: &str, enter: bool) {
    assert_ok(&answer(request::Payload::SendToPane(SendToPane {
        pane_id: pane.into(),
        text: text.into(),
        enter,
        ..SendToPane::default()
    })));
}

#[test]
fn image_completion_uses_its_original_pane_and_precedes_held_typing() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let first = capture(&daemon, "p1");
    let second = capture(&daemon, "p2");
    open_on(&daemon, "");
    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        pane_id: "p1".into(),
        daemon_id: "local".into(),
    })));
    let id = prepare();
    send("p1", "AFTER", true);
    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        pane_id: "p2".into(),
        daemon_id: "local".into(),
    })));
    send("p2", "OTHER", false);
    until(
        "the other pane to receive input",
        || std::fs::read(&second).is_ok_and(|bytes| bytes == b"OTHER"),
        || format!("{:?}", std::fs::read(&second)),
    );
    assert_eq!(std::fs::read(&first).unwrap(), b"");

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.resize(5 << 20, 42);
    assert_ok(&answer(request::Payload::PasteImage(PasteImage {
        png: png.clone(),
        upload_id: id,
        error: String::new(),
    })));
    let cache = daemon.socket_path().with_extension("images");
    until(
        "the uploaded path and typing to arrive",
        || std::fs::read(&first).is_ok_and(|bytes| bytes.ends_with(b"AFTER\r")),
        || format!("{:?}", std::fs::read(&first)),
    );
    let saved = std::fs::read_dir(&cache)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "png"))
        .unwrap();
    assert_eq!(std::fs::read(&saved).unwrap(), png);
    let expected = format!("{}AFTER\r", quoted_image_path(saved.to_str().unwrap()).unwrap());
    assert_eq!(std::fs::read(&first).unwrap(), expected.as_bytes());
    assert_eq!(std::fs::read(&second).unwrap(), b"OTHER");
}

#[test]
fn failed_conversion_releases_return_and_allows_the_next_image() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let heard = capture(&daemon, "p1");
    open_on(&daemon, "");
    let id = prepare();
    send("p1", "draft", true);
    assert_ok(&answer(request::Payload::PasteImage(PasteImage {
        upload_id: id,
        error: "conversion failed".into(),
        png: Vec::new(),
    })));
    until(
        "the failed paste to release typing",
        || std::fs::read(&heard).is_ok_and(|bytes| bytes == b"draft\r"),
        || format!("{:?}", std::fs::read(&heard)),
    );
    let next = prepare();
    assert_ne!(next, id);
    assert_ok(&answer(request::Payload::PasteImage(PasteImage {
        upload_id: next,
        error: "end this test's paste".into(),
        png: Vec::new(),
    })));
}

#[test]
fn a_shell_only_restart_discards_the_image_and_typing_before_conversion_completes() {
    let _turn = muster::testing::fresh_session();
    let mut daemon = Daemon::start_built();
    capture(&daemon, "image-restart-shell");
    open_on(&daemon, "");
    *IMAGE_FINISHED.lock().unwrap() = false;
    muster::ffi::muster_set_event_callback(Some(image_status));
    let id = prepare();
    send("image-restart-shell", "OLD_DRAFT", true);
    let saved = daemon.root().join("daemon.state.json");
    until(
        "the shell pane to be durable",
        || {
            std::fs::read_to_string(&saved)
                .is_ok_and(|text| text.contains("\"image-restart-shell\""))
        },
        || std::fs::read_to_string(&saved).unwrap_or_default(),
    );
    daemon.kill();
    daemon.restart();
    until(
        "the window to observe the shell-only restart",
        || {
            let Some(response::Payload::Window(window)) =
                answer(request::Payload::ReadWindow(muster::proto::ReadWindow::default())).payload
            else {
                return false;
            };
            window
                .daemons
                .iter()
                .any(|daemon| daemon.state == "connected" && daemon.detail.contains("restarted"))
        },
        || "the new shell has not reconnected".into(),
    );
    assert_ok(&answer(request::Payload::PasteImage(PasteImage {
        upload_id: id,
        png: b"\x89PNG\r\n\x1a\nimage".to_vec(),
        error: String::new(),
    })));
    until(
        "the image hold to be canceled",
        || *IMAGE_FINISHED.lock().unwrap(),
        || "no completed image status".into(),
    );
    assert!(
        !daemon.socket_path().with_extension("images").exists(),
        "the old operation uploaded into the replacement daemon"
    );
    send("image-restart-shell", "printf 'NEW_SHELL_READY\\n'", true);
    until(
        "the replacement shell to consume its new input",
        || read("image-restart-shell").lines().any(|line| line.trim() == "NEW_SHELL_READY"),
        || read("image-restart-shell"),
    );
    assert!(!read("image-restart-shell").contains("OLD_DRAFT"));
    muster::ffi::muster_set_event_callback(None);
}

#[derive(Default)]
struct UploadGate {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl Pump for UploadGate {
    fn carry(&self, mut client: UnixStream, daemon: &Path) {
        let Ok(mut upstream) = UnixStream::connect(daemon) else { return };
        let Ok(Some(hello)) = connection::receive::<muster_daemon_proto::Hello>(&mut client) else {
            return;
        };
        if connection::send(&mut upstream, &hello).is_err() {
            return;
        }
        let mut answers = upstream.try_clone().unwrap();
        let mut receiver = client.try_clone().unwrap();
        std::thread::spawn(move || {
            let _ = io::copy(&mut answers, &mut receiver);
            let _ = receiver.shutdown(Shutdown::Both);
        });
        if hello.client == "muster image paste" {
            let mut state = self.state.lock().unwrap();
            state.0 = true;
            self.changed.notify_all();
            while !state.1 {
                state = self.changed.wait(state).unwrap();
            }
        }
        let _ = io::copy(&mut client, &mut upstream);
        let _ = upstream.shutdown(Shutdown::Both);
    }
}

struct ReleaseUpload(Arc<UploadGate>);

impl Drop for ReleaseUpload {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().1 = true;
        self.0.changed.notify_all();
    }
}

#[test]
fn an_upload_blocked_on_writes_does_not_block_another_panes_input() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let first = capture(&daemon, "p1");
    let second = capture(&daemon, "p2");
    let gate = Arc::new(UploadGate::default());
    let release = ReleaseUpload(gate.clone());
    let relay = Relay::start(daemon.root(), daemon.socket_path(), gate.clone());
    open_at(&relay.muster_config());
    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        pane_id: "p1".into(),
        daemon_id: "local".into(),
    })));
    let id = prepare();
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.resize(5 << 20, 42);
    assert_ok(&answer(request::Payload::PasteImage(PasteImage {
        png,
        upload_id: id,
        error: String::new(),
    })));
    until(
        "the upload to meet backpressure",
        || gate.state.lock().unwrap().0,
        || "the image connection has not dialed".into(),
    );
    send("p1", "AFTER", true);
    send("p2", "OTHER", false);
    until(
        "the other pane to receive input while the upload cannot write",
        || std::fs::read(&second).is_ok_and(|bytes| bytes == b"OTHER"),
        || format!("{:?}", std::fs::read(&second)),
    );
    assert_eq!(std::fs::read(&first).unwrap(), b"");
    drop(release);
    until(
        "the image path and typing to arrive after writes resume",
        || {
            std::fs::read(&first)
                .is_ok_and(|bytes| bytes.starts_with(b"\"") && bytes.ends_with(b"AFTER\r"))
        },
        || format!("{:?}", std::fs::read(&first)),
    );
}
