//! The one writer to a pane's program.
//!
//! Keystrokes, pastes, `pane send` text and the answers to the program's own queries all reach
//! the program through one queue per pane, drained by one thread, in the order they were
//! queued. Nothing else writes to a pane's PTY. A queue that is full drops a query answer
//! rather than make the reader wait, which is what stops a program that floods output full of
//! queries while not reading its input from wedging the daemon.
//!
//! Input is encoded here, against the pane's modes as the reader last left them in
//! [`Encoding`], so encoding never waits for a parse or a replay (MIP-3 section 6). The mouse and
//! wheel decisions are Ghostty's, from `Surface.zig`, made once by the side that writes.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_core::input::{Key, KeyAction, KeyEvent, Modifiers, OptionAsAlt};
use muster_vt::{
    EncoderError, KeyEncoder, KeyModes, Mode, MouseAction, MouseButton, MouseEncoder, MouseEvent,
    MouseGeometry, RawKeyEvent, Screen as Active, Terminal, encode_paste, paste_is_safe,
};

use crate::effects::{Reported, Reports};
use crate::pane::PaneIo;
use crate::pty::Grid;
use crate::screen::Cleared;

/// How much may wait for a pane's program to read it.
const QUEUE_DEPTH: usize = 1024;

/// Ghostty's default `mouse-scroll-multiplier`, which the app's `scroll_multiplier` scales.
const PRECISE_MULTIPLIER: f64 = 1.0;
const DISCRETE_MULTIPLIER: f64 = 3.0;

/// A cell's height when no surface has said, so a precise scroll still moves by rows.
const FALLBACK_CELL: f64 = 16.0;

/// Something for a pane's program to read.
#[derive(Debug)]
pub(crate) enum Input {
    /// Bytes as they are: a query's answer, or a report the program asked for.
    Reply(Vec<u8>),
    /// Bytes a person typed that need no encoding, as they are: a `text:`, `csi:` or `esc:`
    /// binding's, and text an input method committed. Typed, where a reply is not.
    Bound(Vec<u8>),
    Key(OwnedKey),
    Mouse(MouseEvent),
    Wheel(Wheel),
    Paste {
        text: String,
        confirmed: bool,
    },
    Send {
        text: String,
        enter: bool,
    },
    Focus(bool),
    /// A full reset of the terminal, as Ghostty's `reset` does: the surface and the daemon's
    /// copy are reset, and the program is not told.
    Reset,
    /// Ghostty's clear_screen: the daemon's terminal cleared and the surface sent the result,
    /// and a shell at its prompt sent a form feed to draw the prompt again. On the alternate
    /// screen nothing is cleared, and the program is sent the key that asked, encoded here
    /// against its modes as if no binding had taken it.
    ClearScreen {
        key: Option<OwnedKey>,
    },
}

impl Input {
    /// What a clear sends the program after it: a form feed, for a shell cleared at its prompt
    /// to draw the prompt again, and on the alternate screen the key that asked. That key is
    /// typed like any other, so its echo is not read as the program at work.
    pub(crate) fn after_clear(cleared: Cleared, key: Option<OwnedKey>) -> Option<Input> {
        match cleared {
            Cleared::AtPrompt => Some(Input::Reply(vec![0x0c])),
            Cleared::Alternate => key.map(Input::Key),
            Cleared::Elsewhere | Cleared::Deferred => None,
        }
    }

    /// Whether this is someone's input, which a program echoes. A reply answers the program
    /// itself, and a focus report, a reset and a clear are not typed.
    fn is_typed(&self) -> bool {
        matches!(
            self,
            Input::Bound(_)
                | Input::Key(_)
                | Input::Mouse(_)
                | Input::Wheel(_)
                | Input::Paste { .. }
                | Input::Send { .. }
        )
    }
}

/// A key as it arrives: libghostty's numbering, and the app's option-as-alt setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnedKey {
    pub(crate) action: KeyAction,
    pub(crate) code: u32,
    pub(crate) modifiers: u16,
    pub(crate) consumed_modifiers: u16,
    pub(crate) text: String,
    pub(crate) unshifted_codepoint: u32,
    pub(crate) composing: bool,
    pub(crate) option_as_alt: OptionAsAlt,
}

/// A wheel or trackpad turn, positive up and to the right as Ghostty's scroll callback takes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Wheel {
    pub(crate) dx: f64,
    pub(crate) dy: f64,
    pub(crate) precise: bool,
    pub(crate) modifiers: Modifiers,
    pub(crate) position: (f32, f32),
}

/// The pane's modes as input needs them, copied out of its terminal after each chunk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // one flag per mode
pub(crate) struct InputModes {
    pub(crate) bracketed_paste: bool,
    pub(crate) alternate_screen: bool,
    pub(crate) alternate_scroll: bool,
    pub(crate) cursor_keys: bool,
    pub(crate) mouse_tracking: bool,
    pub(crate) focus_events: bool,
    /// What the program said with XTSHIFTESCAPE, None while it has said nothing.
    pub(crate) shift_capture: Option<bool>,
}

impl InputModes {
    fn of(terminal: &Terminal) -> InputModes {
        InputModes {
            bracketed_paste: terminal.mode(Mode::BRACKETED_PASTE),
            alternate_screen: terminal.active_screen() == Active::Alternate,
            alternate_scroll: terminal.mode(Mode::ALT_SCROLL),
            cursor_keys: terminal.mode(Mode::CURSOR_KEYS),
            mouse_tracking: terminal.mouse_tracking(),
            focus_events: terminal.mode(Mode::FOCUS_EVENT),
            shift_capture: terminal.mouse_shift_capture(),
        }
    }
}

/// What the writer encodes against. The reader refreshes it under the pane's lock after each
/// chunk; the writer holds it only while encoding, never while writing.
#[derive(Debug)]
pub(crate) struct Encoding {
    key: KeyEncoder,
    mouse: MouseEncoder,
    modes: InputModes,
    grid: Grid,
    scroll_multiplier: f64,
}

impl Encoding {
    pub(crate) fn new(terminal: &Terminal, grid: Grid) -> Result<Encoding, EncoderError> {
        let mut encoding = Encoding {
            key: KeyEncoder::new(KeyModes::default())?,
            mouse: MouseEncoder::new()?,
            modes: InputModes::default(),
            grid,
            scroll_multiplier: 1.0,
        };
        encoding.refresh(terminal);
        Ok(encoding)
    }

    /// Takes the pane's modes from its terminal: the encoders' own (cursor and keypad modes,
    /// modifyOtherKeys, kitty flags, mouse tracking and format), and the ones the decisions
    /// here need.
    /// True when the refresh changed what XTSHIFTESCAPE says, which the pane's record carries.
    pub(crate) fn refresh(&mut self, terminal: &Terminal) -> bool {
        self.key.configure_from(terminal, OptionAsAlt::Never);
        self.mouse.configure_from(terminal);
        let before = self.modes.shift_capture;
        self.modes = InputModes::of(terminal);
        self.modes.shift_capture != before
    }

    pub(crate) fn shift_capture(&self) -> Option<bool> {
        self.modes.shift_capture
    }

    pub(crate) fn resize(&mut self, grid: Grid) {
        self.grid = grid;
    }

    pub(crate) fn set_scroll_multiplier(&mut self, multiplier: f64) {
        self.scroll_multiplier = multiplier;
    }

    fn cell(&self) -> (f64, f64) {
        let cell = |pixels: u16, cells: u16| {
            if pixels == 0 || cells == 0 {
                FALLBACK_CELL
            } else {
                f64::from(pixels) / f64::from(cells)
            }
        };
        (cell(self.grid.width_px, self.grid.cols), cell(self.grid.height_px, self.grid.rows))
    }

    fn geometry(&self) -> MouseGeometry {
        let (width, height) = self.cell();
        MouseGeometry {
            screen_pixels: (u32::from(self.grid.width_px), u32::from(self.grid.height_px)),
            // Whole pixels, which is what a surface's cells are.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            cell_pixels: (width as u32, height as u32),
            padding: (0, 0, 0, 0),
        }
    }
}

pub(crate) fn queue() -> (SyncSender<Input>, Receiver<Input>) {
    mpsc::sync_channel(QUEUE_DEPTH)
}

/// What the writer keeps between inputs.
pub(crate) struct Writer {
    pane: String,
    serial: u64,
    encoding: Arc<Mutex<Encoding>>,
    /// For a reset, which writes to the pane's terminal rather than its program. Weak, since
    /// the pane owns this writer's queue and must be able to go.
    io: Weak<PaneIo>,
    reports: Reports,
    buttons_held: u8,
    scroll: Scroll,
}

/// The bytes of text a program or agent sent. Not a clipboard, so never held; fenced when the
/// program asked, so a newline in it is text rather than a Return. No text is no paste: an
/// empty one is still a paste to the program, which may take a Return straight after it as part
/// of it - a Claude Code prompt was left unsubmitted that way.
fn sent_text(text: String, bracketed_paste: bool) -> Vec<u8> {
    if text.is_empty() {
        Vec::new()
    } else if bracketed_paste {
        encode_paste(&text, true)
    } else {
        text.into_bytes()
    }
}

impl Writer {
    pub(crate) fn new(
        pane: String,
        serial: u64,
        encoding: Arc<Mutex<Encoding>>,
        io: Weak<PaneIo>,
        reports: Reports,
    ) -> Writer {
        Writer { pane, serial, encoding, io, reports, buttons_held: 0, scroll: Scroll::default() }
    }

    /// Writes everything queued for a pane until the pane lets go of its queue, or of its PTY.
    pub(crate) fn write(mut self, queued: &Receiver<Input>, master: &OwnedFd, wake: &OwnedFd) {
        for input in queued {
            // Encoded after it is performed rather than before, since the program may have
            // changed its keyboard modes along with its screen.
            let input = match input {
                Input::Reset | Input::ClearScreen { .. } => match self.perform(input) {
                    Some(sent) => sent,
                    None => continue,
                },
                input => input,
            };
            let typed = input.is_typed();
            // The encoding is released before the write, which can wait on a program that is
            // not reading, so the reader's refresh never waits on it.
            let bytes = self.encode(input);
            if bytes.is_empty() {
                continue;
            }
            if let Err(error) = write_all(master, wake, &bytes) {
                if error.kind() != io::ErrorKind::BrokenPipe {
                    log::warn(
                        "daemon.pane.write_failed",
                        fields! {
                            "pane" => self.pane,
                            "error" => error,
                            "impact" => "the pane's program stops receiving input; its pane is \
                                         closing or its terminal has gone",
                        },
                    );
                }
                return;
            }
            if typed && let Some(io) = self.io.upgrade() {
                io.wrote_input(Instant::now());
            }
        }
    }

    fn encode(&mut self, input: Input) -> Vec<u8> {
        let shared = Arc::clone(&self.encoding);
        let mut encoding = poison::lock(&shared, "daemon.pane.encoding");
        let modes = encoding.modes;
        match input {
            Input::Reply(bytes) | Input::Bound(bytes) => bytes,
            Input::Key(key) => self.key(&mut encoding, &key),
            Input::Mouse(event) => {
                if !modes.mouse_tracking {
                    return Vec::new();
                }
                self.track_buttons(&event);
                // Shift belongs to the surface's selection, as Ghostty's default
                // `mouse-shift-capture` has it, unless the program asked for it (XTSHIFTESCAPE).
                let captured = modes.shift_capture == Some(true);
                let shifted = event.modifiers.0 & Modifiers::SHIFT.0 != 0;
                if shifted && !captured && event.action != MouseAction::Motion {
                    return Vec::new();
                }
                let event = if captured {
                    event
                } else {
                    MouseEvent {
                        modifiers: Modifiers(event.modifiers.0 & !Modifiers::SHIFT.0),
                        ..event
                    }
                };
                let geometry = encoding.geometry();
                encoding.mouse.set_geometry(geometry);
                encoding.mouse.set_button_held(self.buttons_held != 0);
                self.encoded(encoding.mouse.encode(&event))
            }
            Input::Wheel(wheel) => {
                let lines = self.scroll.lines(&wheel, encoding.cell(), encoding.scroll_multiplier);
                match decide(modes, lines) {
                    Wheeled::Arrows(bytes) => bytes,
                    Wheeled::Report(buttons) => {
                        let geometry = encoding.geometry();
                        encoding.mouse.set_geometry(geometry);
                        let mut bytes = Vec::new();
                        for button in buttons {
                            let event = MouseEvent {
                                action: MouseAction::Press,
                                button: Some(button),
                                modifiers: wheel.modifiers,
                                position: wheel.position,
                            };
                            bytes.extend(self.encoded(encoding.mouse.encode(&event)));
                        }
                        bytes
                    }
                    Wheeled::Nothing => Vec::new(),
                }
            }
            Input::Paste { text, confirmed } => {
                if modes.bracketed_paste {
                    return encode_paste(&text, true);
                }
                if !confirmed && !paste_is_safe(&text) {
                    drop(encoding);
                    self.reports.send(self.serial, Reported::PasteHeld(text));
                    return Vec::new();
                }
                encode_paste(&text, false)
            }
            Input::Send { text, enter } => {
                let mut bytes = sent_text(text, modes.bracketed_paste);
                if enter {
                    for action in [KeyAction::Press, KeyAction::Release] {
                        let key = KeyEvent { action, ..KeyEvent::press(Key::Enter) };
                        let encoded = encoding.key.encode(&key);
                        bytes.extend(self.encoded(encoded));
                    }
                }
                bytes
            }
            Input::Focus(focused) => {
                if !modes.focus_events {
                    return Vec::new();
                }
                if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() }
            }
            Input::Reset | Input::ClearScreen { .. } => {
                unreachable!("a reset or a clear is performed before anything is encoded")
            }
        }
    }

    /// What is done to the pane itself rather than encoded for its program, with whatever the
    /// program is to be sent after it ([`Input::after_clear`]).
    fn perform(&self, input: Input) -> Option<Input> {
        let io = self.io.upgrade()?;
        match input {
            Input::Reset => {
                io.reset();
                None
            }
            Input::ClearScreen { key } => Input::after_clear(io.clear_screen(key.as_ref()), key),
            _ => None,
        }
    }

    /// A keystroke, encoded against the pane's modes.
    fn key(&self, encoding: &mut Encoding, key: &OwnedKey) -> Vec<u8> {
        encoding.key.set_option_as_alt(key.option_as_alt);
        let raw = RawKeyEvent {
            action: key.action,
            code: key.code,
            modifiers: key.modifiers,
            consumed_modifiers: key.consumed_modifiers,
            text: &key.text,
            unshifted_codepoint: key.unshifted_codepoint,
            composing: key.composing,
        };
        self.encoded(encoding.key.encode_raw(&raw))
    }

    fn track_buttons(&mut self, event: &MouseEvent) {
        let bit = match event.button {
            Some(MouseButton::Left) => 1,
            Some(MouseButton::Right) => 2,
            Some(MouseButton::Middle) => 4,
            _ => 0,
        };
        match event.action {
            MouseAction::Press => self.buttons_held |= bit,
            MouseAction::Release => self.buttons_held &= !bit,
            MouseAction::Motion => {}
        }
    }

    fn encoded(&self, encoded: Result<Vec<u8>, EncoderError>) -> Vec<u8> {
        encoded.unwrap_or_else(|error| {
            log::warn(
                "daemon.pane.not_encoded",
                fields! {
                    "pane" => self.pane,
                    "error" => error,
                    "impact" => "that keystroke or click did not reach the pane's program",
                },
            );
            Vec::new()
        })
    }
}

/// Wheel movement Ghostty has not yet turned into whole rows or columns.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Scroll {
    pending_x: f64,
    pending_y: f64,
}

impl Scroll {
    /// Whole columns and rows a turn comes to, keeping what is left over, as Ghostty's
    /// `scrollCallback` counts them: precise deltas are pixels, and a discrete tick is at least
    /// one, times the multiplier, rows of the cell's height. `multiplier` scales both after
    /// that rounding, as Ghostty's own does, so one below 1 slows a notch rather than being
    /// rounded away.
    ///
    /// The rounding is Ghostty's on macOS, where a slow notch arrives as a tenth of one, and it
    /// is applied whatever the daemon runs on: a Linux daemon serves a Mac's window over ssh,
    /// and a Linux window reports whole notches, which it leaves alone.
    pub(crate) fn lines(
        &mut self,
        wheel: &Wheel,
        (width, height): (f64, f64),
        multiplier: f64,
    ) -> (i32, i32) {
        let y = if wheel.dy == 0.0 {
            0
        } else {
            let adjusted = if wheel.precise {
                wheel.dy * PRECISE_MULTIPLIER * multiplier
            } else {
                let tick = if wheel.dy > 0.0 { wheel.dy.max(1.0) } else { wheel.dy.min(-1.0) };
                tick * height * DISCRETE_MULTIPLIER * multiplier
            };
            accumulate(&mut self.pending_y, adjusted, height)
        };
        let x = if wheel.dx == 0.0 {
            0
        } else if wheel.precise {
            accumulate(&mut self.pending_x, wheel.dx, width)
        } else {
            whole(wheel.dx.round())
        };
        (x, y)
    }
}

fn accumulate(pending: &mut f64, delta: f64, cell: f64) -> i32 {
    let offset = *pending + delta;
    if offset.abs() < cell {
        *pending = offset;
        return 0;
    }
    let amount = offset / cell;
    *pending = offset - amount * cell;
    whole(amount.trunc())
}

/// A count of rows already rounded, which a wheel never makes large.
#[allow(clippy::cast_possible_truncation)]
fn whole(rounded: f64) -> i32 {
    rounded.clamp(-1000.0, 1000.0) as i32
}

/// What a wheel turn becomes for the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Wheeled {
    /// Cursor keys, for a program on the alternate screen that asked for alternate scroll and
    /// no mouse reports: `less`, `man`, git's pager.
    Arrows(Vec<u8>),
    /// Wheel buttons, for a program tracking the mouse.
    Report(Vec<MouseButton>),
    /// The surface scrolls its own viewport, and the program hears nothing.
    Nothing,
}

/// Ghostty's decision, from `Surface.zig`'s `scrollCallback`.
pub(crate) fn decide(modes: InputModes, (x, y): (i32, i32)) -> Wheeled {
    if modes.alternate_screen && !modes.mouse_tracking && modes.alternate_scroll {
        let arrow: &[u8] = match (y > 0, modes.cursor_keys) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1b[A",
            (false, true) => b"\x1bOB",
            (false, false) => b"\x1b[B",
        };
        return Wheeled::Arrows(arrow.repeat(y.unsigned_abs() as usize));
    }
    if modes.mouse_tracking {
        let vertical = if y > 0 { MouseButton::WheelUp } else { MouseButton::WheelDown };
        let horizontal = if x > 0 { MouseButton::WheelRight } else { MouseButton::WheelLeft };
        let mut buttons = vec![vertical; y.unsigned_abs() as usize];
        buttons.extend(std::iter::repeat_n(horizontal, x.unsigned_abs() as usize));
        return Wheeled::Report(buttons);
    }
    Wheeled::Nothing
}

/// Writes all of `bytes` to the master, waiting while the program is not reading. Gives up
/// with `BrokenPipe` once the pane lets go (its wake pipe closes), so a program that never
/// reads again cannot hold this thread.
fn write_all(master: &OwnedFd, wake: &OwnedFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        // SAFETY: the slice is valid for reads of its length.
        let written =
            unsafe { libc::write(master.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
        if written >= 0 {
            bytes = &bytes[written.cast_unsigned()..];
            continue;
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::Interrupted => {}
            io::ErrorKind::WouldBlock => {
                if !writable(master, wake)? {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
            }
            _ => return Err(error),
        }
    }
    Ok(())
}

/// Waits until the master takes more. False once the pane has let go.
fn writable(master: &OwnedFd, wake: &OwnedFd) -> io::Result<bool> {
    loop {
        let mut watched = [
            libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLOUT, revents: 0 },
            libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: `watched` is a valid array of two pollfds for the length given.
        if unsafe { libc::poll(watched.as_mut_ptr(), 2, -1) } == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if watched[1].revents != 0 {
            return Ok(false);
        }
        if watched[0].revents != 0 {
            return Ok(true);
        }
    }
}

/// A descriptor for the same pipe end, close-on-exec, so the writer can watch the wake pipe
/// beside the reader.
pub(crate) fn duplicate(fd: &OwnedFd) -> io::Result<OwnedFd> {
    use std::os::fd::FromRawFd;
    // SAFETY: F_DUPFD_CLOEXEC on a descriptor this process owns returns a new one or -1.
    let duplicate = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl just returned this descriptor, which nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use muster_daemon_proto::input_event::{self, Input as Event};
    use std::io::{PipeReader, PipeWriter, Read, Write};

    /// A writer for a pane with no program, writing to a pipe whose other end the test reads.
    struct Writing {
        io: Arc<PaneIo>,
        queue: Option<SyncSender<Input>>,
        written: PipeReader,
        wake: PipeWriter,
        thread: Option<std::thread::JoinHandle<()>>,
        _reports: Receiver<crate::effects::Report>,
    }

    impl Writing {
        fn new() -> Writing {
            let io = PaneIo::idle(1);
            let (written, master) = io::pipe().expect("a pipe");
            let master = OwnedFd::from(master);
            // SAFETY: fcntl on a descriptor this test owns.
            unsafe {
                let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
                assert_ne!(
                    libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK),
                    -1
                );
            }
            let (woken, wake) = io::pipe().expect("a pipe");
            let woken = OwnedFd::from(woken);
            let (queue, queued) = queue();
            let (reports, received) = Reports::with_depth(1);
            let writer =
                Writer::new("p1".to_string(), 1, io.encoding(), Arc::downgrade(&io), reports);
            let thread = std::thread::spawn(move || writer.write(&queued, &master, &woken));
            Writing {
                io,
                queue: Some(queue),
                written,
                wake,
                thread: Some(thread),
                _reports: received,
            }
        }

        fn send(&self, input: Input) {
            self.queue.as_ref().expect("a queue").send(input).expect("the writer is running");
        }

        /// Writes `event`, and returns once the writer has finished with it: a reply queued
        /// after it has reached the pipe.
        fn written(&mut self, event: Event) {
            self.send(crate::input::input_of(event).expect("something to write"));
            self.send(Input::Reply(b"<done>".to_vec()));
            let mut seen = Vec::new();
            let mut byte = [0];
            while !seen.ends_with(b"<done>") {
                self.written.read_exact(&mut byte).expect("the writer wrote");
                seen.push(byte[0]);
            }
        }
    }

    impl Drop for Writing {
        fn drop(&mut self) {
            let _ = self.wake.write_all(b"x");
            self.queue = None;
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn key(text: &str) -> Event {
        Event::Key(input_event::Key { text: text.to_string(), ..Default::default() })
    }

    fn bound(bytes: &[u8]) -> Event {
        Event::Perform(input_event::Perform {
            action: Some(input_event::perform::Action::Raw(bytes.to_vec())),
            key: None,
        })
    }

    /// Input someone typed is what a program echoes, and detection takes the echo for it rather
    /// than for the agent's output; what answers the program itself is not.
    #[test]
    fn a_key_counts_as_typed_input_and_a_reply_does_not() {
        let mut writing = Writing::new();
        writing.send(Input::Reply(b"\x1b[?1;2c".to_vec()));
        writing.written(Event::Focus(input_event::Focus { focused: true }));
        assert_eq!(writing.io.input_at(), None, "a reply and a focus report are not typed");
        writing.written(key("a"));
        assert!(writing.io.input_at().is_some(), "a key is typed");
    }

    /// A binding's bytes, such as shift+enter's `text:\n` or option+left's word jump, are
    /// typed as surely as the key they replace.
    #[test]
    fn a_bindings_bytes_count_as_typed_input() {
        let mut writing = Writing::new();
        writing.written(bound(b"\x1bb"));
        assert!(writing.io.input_at().is_some());
    }

    fn wheel(dy: f64, precise: bool) -> Wheel {
        Wheel { dx: 0.0, dy, precise, modifiers: Modifiers::NONE, position: (0.0, 0.0) }
    }

    /// What a clear sends the program after it. On the alternate screen that is the key that
    /// asked, which is typed: its echo is somebody's keystroke, not the program at work. The
    /// form feed a shell is sent to draw its prompt again answers the clear, not a person.
    #[test]
    fn the_key_a_clear_hands_a_program_counts_as_typed_and_the_form_feed_does_not() {
        let key = OwnedKey {
            action: KeyAction::Press,
            code: 30,
            modifiers: 8,
            consumed_modifiers: 0,
            text: "k".to_string(),
            unshifted_codepoint: u32::from('k'),
            composing: false,
            option_as_alt: OptionAsAlt::Never,
        };
        let sent = Input::after_clear(Cleared::Alternate, Some(key.clone()));
        assert!(matches!(&sent, Some(Input::Key(sent)) if *sent == key), "{sent:?}");
        assert!(sent.is_some_and(|sent| sent.is_typed()));

        let redraw = Input::after_clear(Cleared::AtPrompt, Some(key.clone())).unwrap();
        assert!(matches!(&redraw, Input::Reply(bytes) if bytes == &[0x0c]), "{redraw:?}");
        assert!(!redraw.is_typed());
        assert!(Input::after_clear(Cleared::Elsewhere, Some(key)).is_none());
    }

    #[test]
    fn a_discrete_tick_is_three_rows_and_a_precise_turn_waits_for_a_whole_row() {
        let mut scroll = Scroll::default();
        assert_eq!(scroll.lines(&wheel(1.0, false), (10.0, 20.0), 1.0), (0, 3));
        assert_eq!(scroll.lines(&wheel(-1.0, false), (10.0, 20.0), 1.0), (0, -3));
        assert_eq!(scroll.lines(&wheel(15.0, true), (10.0, 20.0), 1.0), (0, 0), "under a row");
        assert_eq!(scroll.lines(&wheel(15.0, true), (10.0, 20.0), 1.0), (0, 1), "thirty pixels");
    }

    /// A slow notch arrives on a Mac as a tenth of one, and counts as a whole one, which the
    /// multiplier then scales: at a half, a notch is a row and a half, so a row, where scaling
    /// first would round it back up to a whole notch and send three.
    #[test]
    fn the_multiplier_scales_a_notch_after_it_is_rounded_up() {
        let mut scroll = Scroll::default();
        assert_eq!(scroll.lines(&wheel(0.1, false), (10.0, 20.0), 1.0), (0, 3), "a slow notch");
        assert_eq!(scroll.lines(&wheel(1.0, false), (10.0, 20.0), 0.5), (0, 1));
        assert_eq!(scroll.lines(&wheel(0.1, false), (10.0, 20.0), 0.5), (0, 1));
        assert_eq!(scroll.lines(&wheel(30.0, true), (10.0, 20.0), 2.0), (0, 3), "sixty pixels");
    }

    #[test]
    fn a_pager_on_the_alternate_screen_gets_arrows_in_its_cursor_key_mode() {
        let pager =
            InputModes { alternate_screen: true, alternate_scroll: true, ..InputModes::default() };
        assert_eq!(decide(pager, (0, 2)), Wheeled::Arrows(b"\x1b[A\x1b[A".to_vec()));
        let application = InputModes { cursor_keys: true, ..pager };
        assert_eq!(decide(application, (0, -1)), Wheeled::Arrows(b"\x1bOB".to_vec()));
    }

    #[test]
    fn a_program_tracking_the_mouse_gets_wheel_buttons_and_otherwise_nothing() {
        let tracking = InputModes {
            mouse_tracking: true,
            alternate_screen: true,
            alternate_scroll: true,
            ..InputModes::default()
        };
        assert_eq!(
            decide(tracking, (1, -2)),
            Wheeled::Report(vec![
                MouseButton::WheelDown,
                MouseButton::WheelDown,
                MouseButton::WheelRight
            ])
        );
        assert_eq!(decide(InputModes::default(), (0, 3)), Wheeled::Nothing);
        let no_alternate_scroll = InputModes { alternate_screen: true, ..InputModes::default() };
        assert_eq!(decide(no_alternate_scroll, (0, 3)), Wheeled::Nothing);
    }
}
