//! A driver over the first sequential concept in the corpus: a case is a list of steps and
//! the expectation is the ordered trace of everything that went out, across every channel.
//! Cases and their reasoning live in corpus/conformance/pane-input.json.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use conformance::{CaseError, Conformance, fields, hex, strings};
use muster_core::input::{
    Key, KeyAction, KeyEvent, Modifiers, PaneChannel, PaneInput, PaneInputSettings, PaneIntent,
    ScrollDirection,
};
use serde_json::{Value, json};
use support::input::{FakeChannel, FakeEncoder, SendRecorder, SlowChannel};

#[test]
fn pane_input_conformance() {
    let corpus = Conformance::load("pane-input.json");

    let ran = corpus.run(|given| {
        let recorder = Arc::new(SendRecorder::default());
        let control: Arc<dyn PaneChannel> =
            Arc::new(FakeChannel::new("control", recorder.clone(), false, true));
        let pane = PaneInput::new(
            control,
            server_channel(given.get("daemon"), &recorder),
            Arc::new(FakeEncoder),
            &PaneInputSettings::default(),
        );

        for step in given.get("steps").and_then(Value::as_array).unwrap_or(&Vec::new()) {
            apply(step, &pane)?;
        }

        // A daemon-encoded intent is delivered off the caller's thread.
        pane.flush();
        let trace: Vec<Value> =
            recorder.sends().iter().map(|(channel, intent)| describe(channel, intent)).collect();
        Ok(fields([("trace", Some(Value::Array(trace)))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// The daemon channel a case asks for, or none.
///
/// Absent means no daemon at all - the degraded arrangement where the app has to guess.
/// `refuses` means reachable but declining, which is the wedged-daemon state and a
/// different path from absence.
fn server_channel(
    given: Option<&Value>,
    recorder: &Arc<SendRecorder>,
) -> Option<Arc<dyn PaneChannel>> {
    let given = given?;
    let refuses = given.get("refuses").and_then(Value::as_bool).unwrap_or(false);
    Some(Arc::new(FakeChannel::new("daemon", recorder.clone(), true, !refuses)))
}

fn apply(step: &Value, pane: &PaneInput) -> Result<(), CaseError> {
    if let Some(send) = step.get("send") {
        let name = send
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| CaseError::new("`send.key` is missing"))?;
        let key = Key::parse(name)
            .ok_or_else(|| CaseError::new(format!("`{name}` is not a W3C key name")))?;
        let modifiers = Modifiers::parse(&strings(send, "modifiers")).ok_or_else(|| {
            CaseError::new("`send.modifiers` names something that is not a modifier")
        })?;
        pane.send(&KeyEvent {
            action: KeyAction::Press,
            key,
            modifiers,
            text: send.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
            ..KeyEvent::default()
        });
        return Ok(());
    }
    if let Some(text) = step.get("paste").and_then(Value::as_str) {
        pane.paste(text);
        return Ok(());
    }
    if let Some(scroll) = step.get("scroll") {
        let direction = scroll
            .get("direction")
            .and_then(Value::as_str)
            .and_then(ScrollDirection::parse)
            .ok_or_else(|| CaseError::new("`scroll` needs a known `direction`"))?;
        let lines = scroll
            .get("lines")
            .and_then(Value::as_u64)
            .and_then(|lines| u16::try_from(lines).ok())
            .ok_or_else(|| CaseError::new("`scroll` needs `lines`"))?;
        pane.scroll(direction, lines);
        return Ok(());
    }
    Err(CaseError::new("a step must be one of send, paste, scroll"))
}

fn describe(channel: &str, intent: &PaneIntent) -> Value {
    match intent {
        PaneIntent::Input(bytes) => fields([
            ("channel", Some(json!(channel))),
            ("intent", Some(json!("input"))),
            ("bytes_hex", Some(json!(hex(bytes)))),
        ]),
        PaneIntent::Text(text) => fields([
            ("channel", Some(json!(channel))),
            ("intent", Some(json!("text"))),
            ("text", Some(json!(text))),
        ]),
        PaneIntent::Key { name } => fields([
            ("channel", Some(json!(channel))),
            ("intent", Some(json!("key"))),
            ("name", Some(json!(name))),
        ]),
        PaneIntent::Scroll { direction, lines } => fields([
            ("channel", Some(json!(channel))),
            ("intent", Some(json!("scroll"))),
            ("direction", Some(json!(direction.as_str()))),
            ("lines", Some(json!(lines))),
        ]),
        PaneIntent::Resize { columns, rows } => fields([
            ("channel", Some(json!(channel))),
            ("intent", Some(json!("resize"))),
            ("columns", Some(json!(columns))),
            ("rows", Some(json!(rows))),
        ]),
    }
}

/// An arrow waits on the daemon, and whoever pressed it must not.
///
/// A server-encoded key is a round trip to the daemon, which herdr answers on the thread that
/// renders every pane - measured at 154 ms at p90 and a 500 ms timeout thirteen times in one
/// busy session. The caller is the window's main thread, so every one of those milliseconds was
/// a window that drew nothing and took no other key. The order still has to hold: a key typed
/// after the arrow reaches the pane after it.
#[test]
fn a_slow_daemon_does_not_hold_up_the_keystroke_or_reorder_the_next() {
    let recorder = Arc::new(SendRecorder::default());
    let control: Arc<dyn PaneChannel> =
        Arc::new(FakeChannel::new("control", recorder.clone(), false, true));
    let daemon: Arc<dyn PaneChannel> =
        Arc::new(SlowChannel::new("daemon", recorder.clone(), Duration::from_millis(300)));
    let pane =
        PaneInput::new(control, Some(daemon), Arc::new(FakeEncoder), &PaneInputSettings::default());

    let started = Instant::now();
    pane.send(&press(Key::ArrowUp, ""));
    pane.send(&press(Key::KeyA, "a"));
    let held_for = started.elapsed();

    let deadline = Instant::now() + Duration::from_secs(5);
    while recorder.sends().len() < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(
        held_for < Duration::from_millis(100),
        "two keystrokes held their caller for {held_for:?} behind a 300 ms daemon"
    );
    let order: Vec<String> =
        recorder.sends().iter().map(|(channel, intent)| format!("{channel} {intent:?}")).collect();
    assert_eq!(
        order,
        vec![r#"daemon Key { name: "up" }"#.to_string(), "control Input([97])".to_string()]
    );
}

fn press(key: Key, text: &str) -> KeyEvent {
    KeyEvent { action: KeyAction::Press, key, text: text.to_string(), ..KeyEvent::default() }
}
