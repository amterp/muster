//! A terminal with no screen: bytes in, grid out.
//!
//! This is the production VT engine running headless - the same code the daemon's own
//! terminals and every ghostty surface run. `docs/testing.md` asks for the user-facing
//! oracle to be the terminal grid computed by that engine rather than by a second
//! implementation written to agree with it, and this is where that comes from.

use std::fmt;

use crate::ffi;
use crate::grid::Cursor;
use crate::modes::Mode;
use crate::state::{Palette, Rgb};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalError {
    CreationFailed(i32),
    ResizeFailed(i32),
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TerminalError::CreationFailed(code) => {
                write!(f, "libghostty-vt would not create a terminal (result {code})")
            }
            TerminalError::ResizeFailed(code) => {
                write!(f, "libghostty-vt would not resize the terminal (result {code})")
            }
        }
    }
}

impl std::error::Error for TerminalError {}

#[derive(Debug)]
pub struct Terminal {
    terminal: ffi::GhosttyTerminal,
}

// SAFETY: the handle is owned by this value and freed only by its Drop, and libghostty-vt
// keeps no thread-local state for a terminal, so moving it to another thread is moving a
// pointer. It is deliberately not Sync: every method reaches the handle through `&self` or
// `&mut self`, and two threads reading one terminal while a third writes it is exactly what
// libghostty requires the embedder to prevent. The daemon owns each pane's terminal under
// that pane's lock, the same external synchronization `KeyEncoder` relies on.
unsafe impl Send for Terminal {}

impl Terminal {
    /// A terminal with grapheme clustering on, which is what the panes Muster mirrors have.
    ///
    /// herdr patches its vendored libghostty-vt to make DEC mode 2027 the default
    /// (`vendor/libghostty-vt.patches.md`, `0001-default-grapheme-cluster-mode`), and stock
    /// libghostty-vt does not. Left off, a ZWJ emoji renders across several cells here and
    /// one cell in the daemon, so a grid read here would describe a screen the user never
    /// saw. Found by the cross-oracle test rather than by reading the patch.
    pub fn new(columns: u16, rows: u16) -> Result<Terminal, TerminalError> {
        Terminal::with_grapheme_clustering(columns, rows, true)
    }

    pub fn with_grapheme_clustering(
        columns: u16,
        rows: u16,
        grapheme_clustering: bool,
    ) -> Result<Terminal, TerminalError> {
        let mut handle: ffi::GhosttyTerminal = std::ptr::null_mut();
        // SAFETY: a null allocator asks for libghostty's default, and the out parameter is
        // a handle we own.
        let result =
            unsafe { ffi::ghostty_terminal_new(std::ptr::null(), &raw mut handle, columns, rows) };
        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || handle.is_null() {
            return Err(TerminalError::CreationFailed(result));
        }

        let terminal = Terminal { terminal: handle };
        // As a reset default rather than a mode written at creation: a program's RIS restores
        // defaults, and a mode that was merely set would be lost to the first one while the
        // surface beside this terminal kept it.
        let mut config = ffi::GhosttyTerminalModeConfig {
            mode: Mode::GRAPHEME_CLUSTER.packed(),
            value: grapheme_clustering,
        };
        // SAFETY: the handle is ours and the pointer is to a local of the type documented
        // for MODE_DEFAULT; libghostty copies it.
        let result = unsafe {
            ffi::ghostty_terminal_set(
                terminal.terminal,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
                (&raw mut config).cast(),
            )
        };
        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS {
            return Err(TerminalError::CreationFailed(result));
        }
        Ok(terminal)
    }

    /// Feeds bytes through the VT parser.
    ///
    /// Never fails, by libghostty's own contract: this input is untrusted by definition, so
    /// malformed sequences are logged and dropped rather than propagated. A frame stream
    /// that has gone wrong shows up as a wrong grid, which is what the snapshot then
    /// catches.
    pub fn write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // SAFETY: the slice is live for the duration of the call and its length is reported
        // honestly.
        unsafe {
            ffi::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr().cast(), bytes.len());
        }
    }

    pub fn resize(&mut self, columns: u16, rows: u16) -> Result<(), TerminalError> {
        // Cell pixel dimensions feed image protocols and size reports, neither of which a
        // headless grid reader has any use for.
        // SAFETY: the handle is ours and the call takes only scalars besides.
        let result = unsafe { ffi::ghostty_terminal_resize(self.terminal, columns, rows, 0, 0) };
        if result == ffi::GhosttyResult_GHOSTTY_SUCCESS {
            Ok(())
        } else {
            Err(TerminalError::ResizeFailed(result))
        }
    }

    /// Tells the terminal the palette the app is drawing with, as a theme switch does.
    ///
    /// This sets the *default* palette: entries a program overrode with OSC 4 keep the
    /// program's color, and every other entry follows the theme.
    pub fn set_default_palette(&mut self, palette: &Palette) {
        let raw = palette.map(Rgb::raw);
        // SAFETY: COLOR_PALETTE reads exactly 256 GhosttyColorRgb from the pointer.
        unsafe {
            ffi::ghostty_terminal_set(
                self.terminal,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                raw.as_ptr().cast(),
            );
        }
    }

    pub(crate) fn handle(&self) -> ffi::GhosttyTerminal {
        self.terminal
    }

    /// Where the cursor sits, and whether the user can see it.
    ///
    /// Part of the screen for snapshot purposes: a frame that paints the right glyphs and
    /// leaves the cursor in the wrong cell is a real rendering bug, and a grid-only oracle
    /// would pass it.
    pub fn cursor(&self) -> Cursor {
        let mut column: u16 = 0;
        let mut row: u16 = 0;
        let mut visible = true;
        // SAFETY: each out pointer is to a local of the type libghostty documents for that
        // data kind - two u16 and a bool.
        unsafe {
            ffi::ghostty_terminal_get(
                self.terminal,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_X,
                (&raw mut column).cast(),
            );
            ffi::ghostty_terminal_get(
                self.terminal,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_Y,
                (&raw mut row).cast(),
            );
            ffi::ghostty_terminal_get(
                self.terminal,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE,
                (&raw mut visible).cast(),
            );
        }
        Cursor { column, row, is_visible: visible }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: the handle was created by `new` and is freed exactly once.
        unsafe { ffi::ghostty_terminal_free(self.terminal) };
    }
}
