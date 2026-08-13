//! A terminal with no screen: bytes in, grid out.
//!
//! This is the production VT engine running headless - the same code the daemon's own
//! terminals and every ghostty surface run. `docs/testing.md` asks for the user-facing
//! oracle to be the terminal grid computed by that engine rather than by a second
//! implementation written to agree with it, and this is where that comes from.

use std::fmt;

use crate::ffi;
use crate::grid::{Cell, Cursor, Grid, Row, Width};

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
        // No option on ghostty_terminal_set reaches DEC modes, so this goes in the way any
        // program would set it.
        if grapheme_clustering {
            terminal.write(b"\x1b[?2027h");
        }
        Ok(terminal)
    }

    /// Feeds bytes through the VT parser.
    ///
    /// Never fails, by libghostty's own contract: this input is untrusted by definition, so
    /// malformed sequences are logged and dropped rather than propagated. A frame stream
    /// that has gone wrong shows up as a wrong grid, which is what the snapshot then
    /// catches.
    pub fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // SAFETY: the slice is live for the duration of the call and its length is reported
        // honestly.
        unsafe {
            ffi::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr().cast(), bytes.len());
        }
    }

    pub fn resize(&self, columns: u16, rows: u16) -> Result<(), TerminalError> {
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

    /// Reads the visible screen.
    ///
    /// The viewport rather than the active area, because the viewport is what a user is
    /// looking at, and that is the thing tests are supposed to assert on.
    pub fn viewport(&self, columns: u16, rows: u16) -> Grid {
        Grid {
            rows: (0..rows)
                .map(|y| Row { cells: (0..columns).filter_map(|x| self.cell(x, y)).collect() })
                .collect(),
            cursor: self.cursor(),
        }
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

    fn cell(&self, column: u16, row: u16) -> Option<Cell> {
        let point = ffi::GhosttyPoint {
            tag: ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_VIEWPORT,
            value: ffi::GhosttyPointValue {
                coordinate: ffi::GhosttyPointCoordinate { x: column, y: u32::from(row) },
            },
        };
        let mut grid_ref = ffi::GhosttyGridRef {
            size: size_of::<ffi::GhosttyGridRef>(),
            node: std::ptr::null_mut(),
            x: 0,
            y: 0,
        };

        // SAFETY: the point is fully initialized and the ref is a local we own. libghostty
        // reads `size` to tell which version of the struct it was handed.
        let found =
            unsafe { ffi::ghostty_terminal_grid_ref(self.terminal, point, &raw mut grid_ref) };
        if found != ffi::GhosttyResult_GHOSTTY_SUCCESS {
            return None;
        }

        let mut raw: ffi::GhosttyCell = 0;
        // SAFETY: the ref was just filled in by libghostty and the out parameter is ours.
        if unsafe { ffi::ghostty_grid_ref_cell(&raw const grid_ref, &raw mut raw) }
            != ffi::GhosttyResult_GHOSTTY_SUCCESS
        {
            return None;
        }

        let mut wide = ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW;
        // SAFETY: the out pointer is to a local of the type documented for CELL_DATA_WIDE.
        unsafe {
            ffi::ghostty_cell_get(
                raw,
                ffi::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE,
                (&raw mut wide).cast(),
            );
        }

        Some(Cell { text: graphemes(&mut grid_ref), width: Width::from_raw(wide) })
    }
}

/// The cell's whole grapheme cluster, not just its first codepoint.
///
/// A snapshot that dropped combining marks would render an agent's output as something the
/// user never saw, and would do it silently.
fn graphemes(grid_ref: &mut ffi::GhosttyGridRef) -> String {
    {
        let mut codepoints = vec![0u32; 8];
        let mut count = 0usize;

        // SAFETY: the buffer is ours and its length is reported honestly; on
        // GHOSTTY_OUT_OF_SPACE libghostty writes the count it needs into `count` instead.
        let mut result = unsafe { read_graphemes(grid_ref, &mut codepoints, &raw mut count) };
        if result == ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
            codepoints = vec![0u32; count];
            // SAFETY: as above, now with the capacity libghostty asked for.
            result = unsafe { read_graphemes(grid_ref, &mut codepoints, &raw mut count) };
        }
        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS {
            return String::new();
        }

        codepoints.iter().take(count).filter_map(|point| char::from_u32(*point)).collect()
    }
}

unsafe fn read_graphemes(
    grid_ref: &mut ffi::GhosttyGridRef,
    codepoints: &mut [u32],
    count: *mut usize,
) -> ffi::GhosttyResult {
    // SAFETY: the caller guarantees `count` points at a usize it owns; the buffer is a live
    // slice for the duration of the call.
    unsafe {
        ffi::ghostty_grid_ref_graphemes(
            &raw const *grid_ref,
            codepoints.as_mut_ptr(),
            codepoints.len(),
            count,
        )
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: the handle was created by `new` and is freed exactly once.
        unsafe { ffi::ghostty_terminal_free(self.terminal) };
    }
}
