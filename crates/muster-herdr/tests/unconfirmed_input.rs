//! Input the daemon may already have acted on is not sent a second way.
//!
//! A server-encoded paste or arrow that times out has been written to herdr whole, and herdr
//! answers late rather than dropping a request (`client.rs`). Falling back to the local
//! encoding then delivers it twice: an arrow moves two lines, and a paste lands once raw and
//! unfenced - which a shell runs line by line - and again fenced when herdr gets to it. Alex's
//! log held 13 such timeouts in one session.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use muster_core::input::{
    EncodeError, KeyEncoding, KeyEvent, PaneChannel, PaneInput, PaneInputSettings, PaneIntent,
};
use muster_herdr::{HerdrClient, HerdrPaneChannel};

#[test]
fn a_paste_the_daemon_took_but_never_answered_is_not_typed_again() {
    let directory = std::env::temp_dir().join(format!("mu-unconfirmed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let socket = directory.join("herdr.sock");

    // A daemon that reads the request and says nothing until well after the client gives up.
    let listener = UnixListener::bind(&socket).expect("bind the silent daemon");
    let silent = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("the client dials");
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).expect("the request arrives whole");
        std::thread::sleep(Duration::from_millis(600));
        line
    });

    let typed = Arc::new(Recorder::default());
    let server = HerdrPaneChannel::new(
        HerdrClient::with_timeout(socket.to_string_lossy(), Duration::from_millis(100)),
        "w1:p1",
    );
    let pane = PaneInput::new(
        Arc::clone(&typed) as Arc<dyn PaneChannel>,
        Some(Arc::new(server)),
        Arc::new(Plain),
        &PaneInputSettings::default(),
    );

    pane.paste("echo one\necho two\n");
    pane.flush();

    let request = silent.join().expect("the silent daemon");
    assert!(request.contains("pane.send_input"), "the paste went to the daemon: {request}");
    assert_eq!(
        typed.sends(),
        Vec::<PaneIntent>::new(),
        "the daemon was handed the paste and may paste it yet, so typing it raw as well \
         delivers it twice"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[derive(Default)]
struct Recorder {
    sends: Mutex<Vec<PaneIntent>>,
}

impl Recorder {
    fn sends(&self) -> Vec<PaneIntent> {
        self.sends.lock().expect("a panicking sender poisoned the recorder").clone()
    }
}

impl PaneChannel for Recorder {
    fn deliver(&self, intent: &PaneIntent) -> bool {
        self.sends.lock().expect("a panicking sender poisoned the recorder").push(intent.clone());
        true
    }

    fn encodes_server_side(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "the pane's control stream"
    }
}

struct Plain;

impl KeyEncoding for Plain {
    fn encode(&self, key: &KeyEvent) -> Result<Vec<u8>, EncodeError> {
        Ok(key.text.as_bytes().to_vec())
    }
}
