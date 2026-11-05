use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use muster_core::input::{ImageInput, InputEvent, InputSink, NotSent};
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::PaneId;
use muster_daemon_client::backend::DaemonInput;
use muster_daemon_client::follow::{Follower, Following, Notice};
use muster_daemon_proto as proto;
use muster_harness::requests::{create, in_new_tab, make, screen_text, until_text};
use muster_harness::{Daemon, PATIENCE, until};

#[derive(Default)]
struct Cancellation {
    entered: AtomicBool,
    released: Mutex<bool>,
    changed: Condvar,
    bootstraps: AtomicUsize,
    input: Mutex<Option<Arc<ImageInput>>>,
}

impl Cancellation {
    fn receive(&self, notice: &Notice) {
        if matches!(notice, Notice::ProcessesReplaced { .. }) {
            let mut released = self.released.lock().unwrap();
            self.entered.store(true, Ordering::Release);
            while !*released {
                released = self.changed.wait(released).unwrap();
            }
            self.input.lock().unwrap().as_ref().unwrap().forget_all();
        }
        if matches!(notice, Notice::Bootstrapped { .. }) {
            self.bootstraps.fetch_add(1, Ordering::Release);
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

struct Release(Arc<Cancellation>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[test]
fn replacement_input_waits_for_cancellation_without_blocking_control_answers() {
    let mut daemon = Daemon::start_built();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    let cancellation = Arc::new(Cancellation::default());
    let release = Release(cancellation.clone());
    let receiving = cancellation.clone();
    let follower = Follower::start(
        Following {
            socket: daemon.socket_path().into(),
            client: "image cancellation test".into(),
            daemon: "local".into(),
            remote: false,
        },
        Arc::new(Mutex::new(Mirror::new())),
        Arc::new(move |notice| receiving.receive(&notice)),
    )
    .unwrap();
    let connection = follower.connection();
    until(
        "the initial snapshot to arrive",
        || cancellation.bootstraps.load(Ordering::Acquire) == 1,
        (),
    );
    let input = Arc::new(ImageInput::new(Arc::new(DaemonInput::new(
        connection.clone(),
        muster_vt::key_code,
        "test daemon".into(),
    ))));
    *cancellation.input.lock().unwrap() = Some(input.clone());
    let pane = PaneId::new("p1");
    let old = input.begin(&pane, |_| {}).unwrap();
    input
        .send(&pane, InputEvent::Send { text: "OLD_DRAFT".into(), keys: Vec::new(), enter: true })
        .unwrap();
    let saved = daemon.root().join("daemon.state.json");
    until(
        "the pane to be durable",
        || std::fs::read_to_string(&saved).is_ok_and(|text| text.contains("\"p1\"")),
        (),
    );
    daemon.kill();
    daemon.restart();
    until(
        "the callback to stop before cancellation",
        || cancellation.entered.load(Ordering::Acquire),
        (),
    );
    let control =
        muster_harness::until_some("the replacement control connection", || connection.control());
    assert_eq!(control.snapshot().wait(PATIENCE).unwrap().outcome(), proto::Outcome::Done);
    assert_eq!(
        connection.send_input(proto::InputEvent {
            pane: "p1".into(),
            input: Some(proto::input_event::Input::Focus(proto::input_event::Focus {
                focused: true
            })),
        }),
        Err(NotSent::NotConnected),
        "replacement input escaped before cancellation"
    );
    drop(release);
    until(
        "cancellation and the next snapshot notice to finish",
        || cancellation.bootstraps.load(Ordering::Acquire) >= 2,
        (),
    );
    assert!(old.cancellation.is_canceled());
    assert!(!input.finish(&pane, old.id, Ok("/tmp/old-image.png".into())));
    input
        .send(
            &pane,
            InputEvent::Send {
                text: "printf 'FRESH_INPUT\\n'".into(),
                keys: Vec::new(),
                enter: true,
            },
        )
        .unwrap();
    until_text(&mut daemon.connect(), "p1", "FRESH_INPUT\n");
    let text = screen_text(&mut daemon.connect(), "p1");
    assert!(!text.contains("OLD_DRAFT") && !text.contains("old-image.png"), "{text}");
}
