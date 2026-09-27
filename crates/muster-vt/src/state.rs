//! What a terminal's state is, read back out.
//!
//! The daemon decides what a program's input means from these (a key, a wheel event, a
//! paste), and composes a replay from them. Each is one `ghostty_terminal_get`, so none of
//! them walks the grid.

use crate::ffi;
use crate::modes::Mode;
use crate::terminal::Terminal;

/// Which of the terminal's two screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Primary,
    Alternate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    fn from_raw(raw: ffi::GhosttyColorRgb) -> Rgb {
        Rgb { r: raw.r, g: raw.g, b: raw.b }
    }

    pub(crate) fn raw(self) -> ffi::GhosttyColorRgb {
        ffi::GhosttyColorRgb { r: self.r, g: self.g, b: self.b }
    }
}

/// The 256 indexed colors.
pub type Palette = [Rgb; 256];

impl Terminal {
    pub fn columns(&self) -> u16 {
        self.get(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLS, 0u16)
    }

    pub fn rows(&self) -> u16 {
        self.get(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ROWS, 0u16)
    }

    /// Whether `mode` is set. A mode the pinned library does not know reads as unset.
    pub fn mode(&self, mode: Mode) -> bool {
        let mut config = ffi::GhosttyTerminalModeConfig { mode: mode.packed(), value: false };
        // SAFETY: MODE is an in/out parameter of exactly this type, with `mode` filled in.
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.handle(),
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MODE,
                (&raw mut config).cast(),
            )
        };
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS && config.value
    }

    /// The kitty keyboard protocol's current flags (`CSI > u` pushes, `CSI < u` pops).
    pub fn kitty_keyboard_flags(&self) -> u8 {
        self.get(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS, 0u8)
    }

    /// Whether any mouse tracking mode is on, so the program wants mouse reports.
    pub fn mouse_tracking(&self) -> bool {
        self.get(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING, false)
    }

    pub fn active_screen(&self) -> Screen {
        let raw = self.get(
            ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
            ffi::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_PRIMARY,
        );
        if raw == ffi::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_ALTERNATE {
            Screen::Alternate
        } else {
            Screen::Primary
        }
    }

    /// Rows above the active area on the active screen.
    pub fn scrollback_rows(&self) -> usize {
        self.get(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS, 0usize)
    }

    /// Scrollback plus the active area.
    pub fn total_rows(&self) -> usize {
        self.get(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TOTAL_ROWS, 0usize)
    }

    /// Whether the cursor sits past the last column, so the next character wraps first.
    pub fn pending_wrap(&self) -> bool {
        self.get(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP, false)
    }

    /// The palette as programs have left it, overrides included.
    pub fn palette(&self) -> Palette {
        self.palette_of(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_PALETTE)
    }

    /// The palette the terminal was configured with, ignoring what programs changed.
    pub fn default_palette(&self) -> Palette {
        self.palette_of(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_PALETTE_DEFAULT)
    }

    /// The effective foreground, background and cursor colors: a program's override, else
    /// the configured default, else none.
    pub fn foreground(&self) -> Option<Rgb> {
        self.color(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND)
    }

    pub fn background(&self) -> Option<Rgb> {
        self.color(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND)
    }

    pub fn cursor_color(&self) -> Option<Rgb> {
        self.color(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_CURSOR)
    }

    pub fn default_foreground(&self) -> Option<Rgb> {
        self.color(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND_DEFAULT)
    }

    pub fn default_background(&self) -> Option<Rgb> {
        self.color(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND_DEFAULT)
    }

    pub fn default_cursor_color(&self) -> Option<Rgb> {
        self.color(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_CURSOR_DEFAULT)
    }

    /// The title programs set with OSC 0 or 2.
    pub fn title(&self) -> String {
        self.string(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TITLE)
    }

    /// The directory programs reported with OSC 7.
    pub fn pwd(&self) -> String {
        self.string(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_PWD)
    }

    fn palette_of(&self, data: ffi::GhosttyTerminalData) -> Palette {
        let mut raw = [ffi::GhosttyColorRgb { r: 0, g: 0, b: 0 }; 256];
        // SAFETY: both palette kinds write exactly 256 GhosttyColorRgb.
        unsafe { ffi::ghostty_terminal_get(self.handle(), data, raw.as_mut_ptr().cast()) };
        raw.map(Rgb::from_raw)
    }

    fn color(&self, data: ffi::GhosttyTerminalData) -> Option<Rgb> {
        let mut raw = ffi::GhosttyColorRgb { r: 0, g: 0, b: 0 };
        // SAFETY: every color kind writes one GhosttyColorRgb, and answers NO_VALUE when
        // nothing is set.
        let result =
            unsafe { ffi::ghostty_terminal_get(self.handle(), data, (&raw mut raw).cast()) };
        (result == ffi::GhosttyResult_GHOSTTY_SUCCESS).then(|| Rgb::from_raw(raw))
    }

    fn string(&self, data: ffi::GhosttyTerminalData) -> String {
        let mut raw = ffi::GhosttyString { ptr: std::ptr::null(), len: 0 };
        // SAFETY: TITLE and PWD write a borrowed GhosttyString, valid until the terminal is
        // next written to - which cannot happen while `&self` is held.
        let result =
            unsafe { ffi::ghostty_terminal_get(self.handle(), data, (&raw mut raw).cast()) };
        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || raw.ptr.is_null() {
            return String::new();
        }
        // SAFETY: libghostty reports the borrowed bytes' true length.
        let bytes = unsafe { std::slice::from_raw_parts(raw.ptr, raw.len) };
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// One scalar read. `fallback` is what a read the library refuses returns, and fixes
    /// the out parameter's type - which must be the one terminal.h documents for `data`.
    fn get<T: Copy>(&self, data: ffi::GhosttyTerminalData, fallback: T) -> T {
        let mut value = fallback;
        // SAFETY: every caller pairs `data` with the output type the header documents for it.
        let result =
            unsafe { ffi::ghostty_terminal_get(self.handle(), data, (&raw mut value).cast()) };
        if result == ffi::GhosttyResult_GHOSTTY_SUCCESS { value } else { fallback }
    }
}
