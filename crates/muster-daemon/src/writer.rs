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

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_core::input::{Key, KeyAction, KeyEvent, Modifiers, OptionAsAlt, TerminalModeProfile};
use muster_vt::{
    EncoderError, KeyEncoder, Mode, MouseAction, MouseButton, MouseEncoder, MouseEvent,
    MouseGeometry, RawKeyEvent, Screen as Active, Terminal, encode_paste, paste_is_safe,
};

use crate::effects::{Reported, Reports};
use crate::pane::PaneIo;
use crate::pty::Grid;
use crate::screen::Cleared;

/// How much may wait for a pane's program to read it.
const QUEUE_DEPTH: usize = 1024;

/// Ghostty's default `mouse-scroll-multiplier`.
const PRECISE_MULTIPLIER: f64 = 1.0;
const DISCRETE_MULTIPLIER: f64 = 3.0;

/// A cell's height when no surface has said, so a precise scroll still moves by rows.
const FALLBACK_CELL: f64 = 16.0;

/// Something for a pane's program to read.
#[derive(Debug)]
pub(crate) enum Input {
    /// Bytes as they are: a query's answer, a report the program asked for, or a binding's
    /// resolved bytes.
    Reply(Vec<u8>),
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
    /// screen nothing is cleared, and the program is sent `unconsumed`, the key's own bytes.
    ClearScreen {
        unconsumed: Vec<u8>,
    },
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
}

impl Encoding {
    pub(crate) fn new(terminal: &Terminal, grid: Grid) -> Result<Encoding, EncoderError> {
        let mut encoding = Encoding {
            key: KeyEncoder::new(TerminalModeProfile::default())?,
            mouse: MouseEncoder::new()?,
            modes: InputModes::default(),
            grid,
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
        }
    }

    fn encode(&mut self, input: Input) -> Vec<u8> {
        let shared = Arc::clone(&self.encoding);
        let mut encoding = poison::lock(&shared, "daemon.pane.encoding");
        let modes = encoding.modes;
        match input {
            Input::Reply(bytes) => bytes,
            Input::Key(key) => {
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
                let lines = self.scroll.lines(&wheel, encoding.cell());
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
                // A program or agent speaking, not a clipboard, so it is never held; fenced
                // when the program asked, so a newline in it is text rather than a Return.
                let mut bytes = if modes.bracketed_paste {
                    encode_paste(&text, true)
                } else {
                    text.into_bytes()
                };
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
                drop(encoding);
                self.perform(&input)
            }
        }
    }

    /// What is done to the pane itself rather than encoded for its program, with whatever the
    /// program is to be sent after it: a form feed, for a shell whose screen was cleared at its
    /// prompt, to draw the prompt again.
    fn perform(&self, input: &Input) -> Vec<u8> {
        let Some(io) = self.io.upgrade() else { return Vec::new() };
        match input {
            Input::Reset => io.reset(),
            Input::ClearScreen { unconsumed } => {
                return match io.clear_screen(unconsumed) {
                    Cleared::AtPrompt => vec![0x0c],
                    Cleared::Alternate => unconsumed.clone(),
                    Cleared::Elsewhere | Cleared::Deferred => Vec::new(),
                };
            }
            _ => {}
        }
        Vec::new()
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
    /// one, times the multiplier, rows of the cell's height.
    pub(crate) fn lines(&mut self, wheel: &Wheel, (width, height): (f64, f64)) -> (i32, i32) {
        let y = if wheel.dy == 0.0 {
            0
        } else {
            let adjusted = if wheel.precise {
                wheel.dy * PRECISE_MULTIPLIER
            } else {
                let tick = if cfg!(target_os = "macos") {
                    if wheel.dy > 0.0 { wheel.dy.max(1.0) } else { wheel.dy.min(-1.0) }
                } else {
                    wheel.dy
                };
                tick * height * DISCRETE_MULTIPLIER
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

    fn wheel(dy: f64, precise: bool) -> Wheel {
        Wheel { dx: 0.0, dy, precise, modifiers: Modifiers::NONE, position: (0.0, 0.0) }
    }

    #[test]
    fn a_discrete_tick_is_three_rows_and_a_precise_turn_waits_for_a_whole_row() {
        let mut scroll = Scroll::default();
        assert_eq!(scroll.lines(&wheel(1.0, false), (10.0, 20.0)), (0, 3));
        assert_eq!(scroll.lines(&wheel(-1.0, false), (10.0, 20.0)), (0, -3));
        assert_eq!(scroll.lines(&wheel(15.0, true), (10.0, 20.0)), (0, 0), "under a row");
        assert_eq!(scroll.lines(&wheel(15.0, true), (10.0, 20.0)), (0, 1), "thirty pixels");
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
