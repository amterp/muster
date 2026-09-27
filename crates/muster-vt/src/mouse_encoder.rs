//! Turns a mouse event into the report a terminal program asked for, or into nothing.
//!
//! libghostty-vt's own mouse encoder, configured from the pane's terminal, so the tracking
//! mode and report format are the ones the program set rather than a guess. Whether a wheel
//! turn becomes arrow keys instead (alternate scroll) is the daemon's decision, made from
//! the terminal's modes; this only encodes reports.

use muster_core::input::Modifiers;

use crate::ffi;
use crate::key_encoder::EncoderError;
use crate::terminal::Terminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

/// Buttons as X numbers them: 4 and 5 are the wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseEvent {
    pub action: MouseAction,
    /// None for motion with no button held.
    pub button: Option<MouseButton>,
    pub modifiers: Modifiers,
    /// In pixels from the top-left of the surface.
    pub position: (f32, f32),
}

/// The surface's geometry, which turns a pixel position into a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseGeometry {
    pub screen_pixels: (u32, u32),
    /// Must be non-zero.
    pub cell_pixels: (u32, u32),
    /// Top, right, bottom, left.
    pub padding: (u32, u32, u32, u32),
}

#[derive(Debug)]
pub struct MouseEncoder {
    encoder: ffi::GhosttyMouseEncoder,
    event: ffi::GhosttyMouseEvent,
}

// SAFETY: as for `KeyEncoder`: both handles are owned by this value and reached only through
// its own methods, so libghostty-vt never sees two callers at once.
unsafe impl Send for MouseEncoder {}

impl MouseEncoder {
    pub fn new() -> Result<MouseEncoder, EncoderError> {
        let mut encoder: ffi::GhosttyMouseEncoder = std::ptr::null_mut();
        // SAFETY: a null allocator asks for libghostty's default; the out parameter is ours.
        let created = unsafe { ffi::ghostty_mouse_encoder_new(std::ptr::null(), &raw mut encoder) };
        if created != ffi::GhosttyResult_GHOSTTY_SUCCESS || encoder.is_null() {
            return Err(EncoderError::CreationFailed(created));
        }
        let mut event: ffi::GhosttyMouseEvent = std::ptr::null_mut();
        // SAFETY: as above.
        let created = unsafe { ffi::ghostty_mouse_event_new(std::ptr::null(), &raw mut event) };
        if created != ffi::GhosttyResult_GHOSTTY_SUCCESS || event.is_null() {
            // SAFETY: created above and freed exactly once here.
            unsafe { ffi::ghostty_mouse_encoder_free(encoder) };
            return Err(EncoderError::CreationFailed(created));
        }
        Ok(MouseEncoder { encoder, event })
    }

    /// Takes the tracking mode and report format from the pane's terminal. Geometry and
    /// held buttons are left as they were.
    pub fn configure_from(&mut self, terminal: &Terminal) {
        // SAFETY: both handles are live; libghostty only reads the terminal.
        unsafe { ffi::ghostty_mouse_encoder_setopt_from_terminal(self.encoder, terminal.handle()) };
    }

    pub fn set_geometry(&mut self, geometry: MouseGeometry) {
        let (top, right, bottom, left) = geometry.padding;
        let size = ffi::GhosttyMouseEncoderSize {
            size: size_of::<ffi::GhosttyMouseEncoderSize>(),
            screen_width: geometry.screen_pixels.0,
            screen_height: geometry.screen_pixels.1,
            cell_width: geometry.cell_pixels.0.max(1),
            cell_height: geometry.cell_pixels.1.max(1),
            padding_top: top,
            padding_bottom: bottom,
            padding_right: right,
            padding_left: left,
        };
        // SAFETY: SIZE reads one GhosttyMouseEncoderSize, which libghostty copies.
        unsafe {
            ffi::ghostty_mouse_encoder_setopt(
                self.encoder,
                ffi::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                (&raw const size).cast(),
            );
        }
    }

    /// Whether a button is held, which decides whether button-event tracking reports motion.
    pub fn set_button_held(&mut self, held: bool) {
        // SAFETY: ANY_BUTTON_PRESSED reads one bool.
        unsafe {
            ffi::ghostty_mouse_encoder_setopt(
                self.encoder,
                ffi::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                (&raw const held).cast(),
            );
        }
    }

    /// The report for this event. Empty is a normal answer: the program asked for no report
    /// of this kind.
    pub fn encode(&mut self, event: &MouseEvent) -> Result<Vec<u8>, EncoderError> {
        // SAFETY: every setter takes the event handle this value owns, and copies its value.
        unsafe {
            ffi::ghostty_mouse_event_set_action(self.event, action(event.action));
            match event.button {
                Some(button) => ffi::ghostty_mouse_event_set_button(self.event, raw_button(button)),
                None => ffi::ghostty_mouse_event_clear_button(self.event),
            }
            ffi::ghostty_mouse_event_set_mods(self.event, event.modifiers.0);
            ffi::ghostty_mouse_event_set_position(
                self.event,
                ffi::GhosttyMousePosition { x: event.position.0, y: event.position.1 },
            );
        }

        let mut buffer = vec![0u8; 64];
        let mut length = 0usize;
        // SAFETY: the buffer is ours and its length is honest; on OUT_OF_SPACE libghostty
        // writes the length it needs instead.
        let mut result = unsafe { self.encode_into(&mut buffer, &raw mut length) };
        if result == ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
            buffer = vec![0u8; length];
            // SAFETY: as above, with the capacity libghostty asked for.
            result = unsafe { self.encode_into(&mut buffer, &raw mut length) };
        }
        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS {
            return Err(EncoderError::EncodingFailed(result));
        }
        buffer.truncate(length);
        Ok(buffer)
    }

    unsafe fn encode_into(&self, buffer: &mut [u8], length: *mut usize) -> ffi::GhosttyResult {
        // SAFETY: the caller guarantees `length` points at a usize it owns; the buffer is a
        // live slice for the duration of the call.
        unsafe {
            ffi::ghostty_mouse_encoder_encode(
                self.encoder,
                self.event,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                length,
            )
        }
    }
}

impl Drop for MouseEncoder {
    fn drop(&mut self) {
        // SAFETY: both handles were created by `new` and are freed exactly once.
        unsafe {
            ffi::ghostty_mouse_event_free(self.event);
            ffi::ghostty_mouse_encoder_free(self.encoder);
        }
    }
}

fn action(action: MouseAction) -> ffi::GhosttyMouseAction {
    match action {
        MouseAction::Press => ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_PRESS,
        MouseAction::Release => ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_RELEASE,
        MouseAction::Motion => ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_MOTION,
    }
}

fn raw_button(button: MouseButton) -> ffi::GhosttyMouseButton {
    match button {
        MouseButton::Left => ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_LEFT,
        MouseButton::Right => ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_RIGHT,
        MouseButton::Middle => ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_MIDDLE,
        MouseButton::WheelUp => ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FOUR,
        MouseButton::WheelDown => ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FIVE,
        MouseButton::WheelLeft => ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_SIX,
        MouseButton::WheelRight => ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_SEVEN,
    }
}
