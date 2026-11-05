use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::diagnostics::poison;
use crate::mirror::backend::PaneId;

use super::image::quoted_image_path;
use super::{InputEvent, InputSink, Key, KeyAction, Modifiers, NotSent};

const HELD_BYTES: usize = 64 << 10;
const HELD_EVENTS: usize = 512;

type Finished = Box<dyn FnOnce(Result<(), String>) + Send>;
type Stop = Box<dyn FnOnce() + Send>;

#[derive(Default)]
pub struct ImageCancellation {
    canceled: AtomicBool,
    stop: Mutex<Option<Stop>>,
}

impl std::fmt::Debug for ImageCancellation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageCancellation").field("canceled", &self.is_canceled()).finish()
    }
}

impl ImageCancellation {
    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }

    pub fn on_cancel(&self, stop: impl FnOnce() + Send + 'static) {
        let mut registered = poison::lock(&self.stop, "image cancellation");
        if self.is_canceled() {
            drop(registered);
            stop();
        } else {
            *registered = Some(Box::new(stop));
        }
    }

    fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
        let stop = poison::lock(&self.stop, "image cancellation").take();
        if let Some(stop) = stop {
            stop();
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageUpload {
    pub id: u64,
    pub cancellation: Arc<ImageCancellation>,
}

struct Held {
    upload: ImageUpload,
    input: VecDeque<InputEvent>,
    bytes: usize,
    finished: Finished,
}

#[derive(Default)]
struct State {
    next: u64,
    held: BTreeMap<PaneId, Held>,
}

pub struct ImageInput {
    sink: Arc<dyn InputSink>,
    state: Mutex<State>,
}

impl std::fmt::Debug for ImageInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageInput").field("sink", &self.sink.description()).finish_non_exhaustive()
    }
}

impl ImageInput {
    pub fn new(sink: Arc<dyn InputSink>) -> ImageInput {
        ImageInput { sink, state: Mutex::new(State::default()) }
    }

    pub fn begin(
        &self,
        pane: &PaneId,
        finished: impl FnOnce(Result<(), String>) + Send + 'static,
    ) -> Result<ImageUpload, String> {
        let mut state = poison::lock(&self.state, "image input");
        if state.held.contains_key(pane) {
            return Err(
                "an image is already being pasted into this pane; try again when it finishes"
                    .into(),
            );
        }
        state.next += 1;
        let upload =
            ImageUpload { id: state.next, cancellation: Arc::new(ImageCancellation::default()) };
        state.held.insert(
            pane.clone(),
            Held {
                upload: upload.clone(),
                input: VecDeque::new(),
                bytes: 0,
                finished: Box::new(finished),
            },
        );
        Ok(upload)
    }

    pub fn finish(&self, pane: &PaneId, id: u64, result: Result<String, String>) -> bool {
        let mut state = poison::lock(&self.state, "image input");
        if state.held.get(pane).is_none_or(|held| held.upload.id != id) {
            return false;
        }
        let held = state.held.remove(pane).expect("the upload was found");
        let result = result.and_then(|path| quoted_image_path(&path)).and_then(|text| {
            self.sink
                .send(pane, InputEvent::Paste { text, confirmed: false })
                .map_err(|why| format!("the image path was not sent: {why}"))
        });
        self.release(pane, held.input);
        drop(state);
        (held.finished)(result);
        true
    }

    pub fn forget(&self, pane: &PaneId) {
        let held = poison::lock(&self.state, "image input").held.remove(pane);
        if let Some(held) = held {
            held.upload.cancellation.cancel();
            (held.finished)(Err(
                "the pane closed or restarted; its image paste was canceled".into()
            ));
        }
    }

    fn release(&self, pane: &PaneId, input: VecDeque<InputEvent>) {
        for event in input {
            let _ = self.sink.send(pane, event);
        }
    }
}

impl InputSink for ImageInput {
    fn send(&self, pane: &PaneId, event: InputEvent) -> Result<(), NotSent> {
        if !held_input(&event) {
            return self.sink.send(pane, event);
        }
        let mut state = poison::lock(&self.state, "image input");
        let Some(held) = state.held.get_mut(pane) else {
            return self.sink.send(pane, event);
        };
        let bytes = input_bytes(&event);
        let interrupted = interrupts(&event);
        if !interrupted && held.input.len() < HELD_EVENTS && held.bytes + bytes <= HELD_BYTES {
            held.bytes += bytes;
            held.input.push_back(event);
            return Ok(());
        }
        let held = state.held.remove(pane).expect("the upload was found");
        self.release(pane, held.input);
        let sent = self.sink.send(pane, event);
        drop(state);
        held.upload.cancellation.cancel();
        let reason = if interrupted {
            "the image paste was canceled by Esc or Ctrl+C"
        } else {
            "too much input arrived during the image paste; the image was canceled and typing was released"
        };
        (held.finished)(Err(reason.into()));
        sent
    }

    fn description(&self) -> &str {
        self.sink.description()
    }
}

fn held_input(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Key { .. }
            | InputEvent::Paste { .. }
            | InputEvent::Send { .. }
            | InputEvent::Bytes(_)
    )
}

fn interrupts(event: &InputEvent) -> bool {
    match event {
        InputEvent::Key { key, .. } if key.action != KeyAction::Release => {
            key.key == Key::Escape
                || (key.key == Key::KeyC
                    && key.modifiers.0 & Modifiers::CHORD.0 == Modifiers::CONTROL.0)
        }
        _ => false,
    }
}

fn input_bytes(event: &InputEvent) -> usize {
    match event {
        InputEvent::Key { key, .. } => key.text.len() + key.text_without_option.len(),
        InputEvent::Paste { text, .. } => text.len(),
        InputEvent::Send { text, keys, .. } => {
            text.len() + keys.iter().map(String::len).sum::<usize>()
        }
        InputEvent::Bytes(bytes) => bytes.len(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{KeyEvent, OptionAsAlt, Wheel};

    #[derive(Debug, Default)]
    struct Sink(Mutex<Vec<(PaneId, InputEvent)>>);

    impl InputSink for Sink {
        fn send(&self, pane: &PaneId, event: InputEvent) -> Result<(), NotSent> {
            self.0.lock().unwrap().push((pane.clone(), event));
            Ok(())
        }

        fn description(&self) -> &'static str {
            "the test's input receiver"
        }
    }

    struct Graph {
        input: ImageInput,
        sink: Arc<Sink>,
        finished: Arc<Mutex<Vec<Result<(), String>>>>,
        pane: PaneId,
    }

    impl Graph {
        fn new() -> Graph {
            let sink = Arc::new(Sink::default());
            Graph {
                input: ImageInput::new(sink.clone()),
                sink,
                finished: Arc::default(),
                pane: PaneId::new("p1"),
            }
        }

        fn begin(&self) -> ImageUpload {
            let finished = self.finished.clone();
            self.input
                .begin(&self.pane, move |result| finished.lock().unwrap().push(result))
                .unwrap()
        }

        fn send(&self, event: InputEvent) {
            self.input.send(&self.pane, event).unwrap();
        }

        fn received(&self) -> Vec<(PaneId, InputEvent)> {
            self.sink.0.lock().unwrap().clone()
        }
    }

    fn key(key: Key, modifiers: Modifiers) -> InputEvent {
        InputEvent::Key {
            key: KeyEvent { key, modifiers, action: KeyAction::Press, ..KeyEvent::default() },
            option_as_alt: OptionAsAlt::Never,
        }
    }

    #[test]
    fn the_image_precedes_held_typing_while_another_pane_keeps_receiving() {
        let graph = Graph::new();
        let upload = graph.begin();
        let typing = InputEvent::Bytes(b"explain this".to_vec());
        let enter = key(Key::Enter, Modifiers::NONE);
        graph.send(typing.clone());
        graph.send(enter.clone());
        let other = PaneId::new("p2");
        graph.input.send(&other, InputEvent::Bytes(b"other".to_vec())).unwrap();
        assert_eq!(graph.received(), [(other.clone(), InputEvent::Bytes(b"other".to_vec()))]);
        assert!(graph.input.finish(&graph.pane, upload.id, Ok("/tmp/image.png".into())));
        assert_eq!(
            graph.received(),
            [
                (other, InputEvent::Bytes(b"other".to_vec())),
                (
                    graph.pane.clone(),
                    InputEvent::Paste { text: "\"/tmp/image.png\"".into(), confirmed: false }
                ),
                (graph.pane.clone(), typing),
                (graph.pane.clone(), enter),
            ]
        );
        assert_eq!(*graph.finished.lock().unwrap(), [Ok(())]);
        assert!(!graph.input.finish(&graph.pane, upload.id, Ok("/tmp/late.png".into())));
    }

    #[test]
    fn failure_releases_a_held_return_without_an_image() {
        let graph = Graph::new();
        let upload = graph.begin();
        let enter = key(Key::Enter, Modifiers::NONE);
        graph.send(enter.clone());
        graph.input.finish(&graph.pane, upload.id, Err("disk full".into()));
        assert_eq!(graph.received(), [(graph.pane.clone(), enter)]);
        assert_eq!(*graph.finished.lock().unwrap(), [Err("disk full".into())]);
        graph.begin();
    }

    #[test]
    fn escape_and_control_c_cancel_without_waiting_for_upload_completion() {
        for cancel in [key(Key::Escape, Modifiers::NONE), key(Key::KeyC, Modifiers::CONTROL)] {
            let graph = Graph::new();
            let upload = graph.begin();
            let typing = InputEvent::Bytes(b"draft".to_vec());
            graph.send(typing.clone());
            graph.send(cancel.clone());
            assert!(upload.cancellation.is_canceled());
            assert_eq!(
                graph.received(),
                [(graph.pane.clone(), typing), (graph.pane.clone(), cancel)]
            );
            assert!(!graph.input.finish(&graph.pane, upload.id, Ok("/tmp/late.png".into())));
            assert_eq!(graph.finished.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn scrolling_is_neither_held_nor_charged_to_the_typing_budget() {
        let graph = Graph::new();
        let upload = graph.begin();
        for _ in 0..1000 {
            graph.send(InputEvent::Wheel(Wheel {
                dx: 0.0,
                dy: 1.0,
                precise: true,
                momentum: 0,
                modifiers: Modifiers::NONE,
                x: 0.0,
                y: 0.0,
            }));
        }
        assert_eq!(graph.received().len(), 1000);
        assert!(!upload.cancellation.is_canceled());
        assert!(graph.input.finish(&graph.pane, upload.id, Ok("/tmp/image.png".into())));
    }

    #[test]
    fn exceeding_either_hold_bound_cancels_the_image_without_dropping_typing() {
        let graph = Graph::new();
        let upload = graph.begin();
        let large = vec![b'x'; 65 << 10];
        graph.send(InputEvent::Bytes(large.clone()));
        assert!(upload.cancellation.is_canceled());
        assert_eq!(graph.received(), [(graph.pane.clone(), InputEvent::Bytes(large))]);

        let graph = Graph::new();
        let upload = graph.begin();
        for _ in 0..513 {
            graph.send(key(Key::KeyA, Modifiers::NONE));
        }
        assert!(upload.cancellation.is_canceled());
        assert_eq!(graph.received().len(), 513);
    }

    #[test]
    fn forgetting_a_pane_discards_its_draft_and_late_result_before_a_new_upload() {
        let graph = Graph::new();
        let old = graph.begin();
        graph.send(InputEvent::Bytes(b"old draft".to_vec()));
        assert!(graph.input.begin(&graph.pane, |_| {}).is_err());
        graph.input.forget(&graph.pane);
        let new = graph.begin();
        assert!(!graph.input.finish(&graph.pane, old.id, Ok("/tmp/old.png".into())));
        assert!(graph.input.finish(&graph.pane, new.id, Ok("/tmp/new.png".into())));
        assert_eq!(
            graph.received(),
            [(
                graph.pane.clone(),
                InputEvent::Paste { text: "\"/tmp/new.png\"".into(), confirmed: false }
            )]
        );
    }
}
