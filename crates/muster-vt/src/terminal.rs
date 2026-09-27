//! A terminal with no screen: bytes in, grid out.
//!
//! This is the production VT engine running headless - the same code the daemon's own
//! terminals and every ghostty surface run. `docs/testing.md` asks for the user-facing
//! oracle to be the terminal grid computed by that engine rather than by a second
//! implementation written to agree with it, and this is where that comes from.

use std::ffi::c_void;
use std::fmt;

use crate::effects::{self, Answers, Effect, Effects};
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

/// How a terminal is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalOptions {
    pub columns: u16,
    pub rows: u16,
    /// One cell's width and height in pixels, for size reports and images. Zero until a
    /// surface says.
    pub cell_pixels: (u32, u32),
    /// DEC mode 2027 as the reset default. On, as herdr's patched libghostty and Ghostty's
    /// own default config have it, so detection manifests see graphemes as they were written
    /// against.
    pub grapheme_clustering: bool,
    /// The most scrollback kept, in bytes. None keeps libghostty's default.
    pub scrollback_bytes: Option<usize>,
    /// What an XTGETTCAP query for `TN` answers, which must agree with the pane's `TERM`.
    pub terminfo_name: Option<String>,
    /// Bytes of kitty graphics kept to answer a program's image queries. Zero turns the
    /// protocol off; None keeps libghostty's default.
    pub kitty_image_bytes: Option<u64>,
    pub answers: Answers,
}

impl TerminalOptions {
    pub fn new(columns: u16, rows: u16) -> TerminalOptions {
        TerminalOptions {
            columns,
            rows,
            cell_pixels: (0, 0),
            grapheme_clustering: true,
            scrollback_bytes: None,
            terminfo_name: None,
            kitty_image_bytes: None,
            answers: Answers::default(),
        }
    }
}

pub struct Terminal {
    terminal: ffi::GhosttyTerminal,
    /// Reached by libghostty's callbacks through the userdata pointer, so it lives on the heap
    /// at an address that holds while the terminal moves, and is freed after the handle.
    ///
    /// Held as the raw pointer libghostty was given rather than as a `Box`: writing through a
    /// `Box` after handing out a pointer derived from it invalidates that pointer under
    /// Stacked Borrows, so every access, ours and the callbacks', goes through this one.
    effects: *mut Effects,
}

impl fmt::Debug for Terminal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Terminal").field("terminal", &self.terminal).finish_non_exhaustive()
    }
}

// SAFETY: the handle is owned by this value and freed only by its Drop, and libghostty-vt
// keeps no thread-local state for a terminal, so moving it to another thread is moving a
// pointer; the effect handler it calls into is Send by its type. It is deliberately not
// Sync: every method reaches the handle through `&self` or `&mut self`, and two threads
// reading one terminal while a third writes it is exactly what libghostty requires the
// embedder to prevent. The daemon owns each pane's terminal under that pane's lock, the same
// external synchronization `KeyEncoder` relies on.
unsafe impl Send for Terminal {}

impl Terminal {
    /// A terminal with grapheme clustering on, which is what the panes Muster mirrors have.
    pub fn new(columns: u16, rows: u16) -> Result<Terminal, TerminalError> {
        Terminal::with_options(TerminalOptions::new(columns, rows))
    }

    pub fn with_options(options: TerminalOptions) -> Result<Terminal, TerminalError> {
        let mut handle: ffi::GhosttyTerminal = std::ptr::null_mut();
        // SAFETY: a null allocator asks for libghostty's default, and the out parameter is
        // a handle we own.
        let result = unsafe {
            ffi::ghostty_terminal_new(
                std::ptr::null(),
                &raw mut handle,
                options.columns,
                options.rows,
            )
        };
        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || handle.is_null() {
            return Err(TerminalError::CreationFailed(result));
        }

        let mut terminal = Terminal {
            terminal: handle,
            effects: Box::into_raw(Box::new(Effects { handler: None, answers: options.answers })),
        };
        // SAFETY: the allocation is owned by the terminal and outlives the handle, which Drop
        // frees first.
        unsafe { effects::register(handle, terminal.effects) };

        // As a reset default rather than a mode written at creation: a program's RIS restores
        // defaults, and a mode that was merely set would be lost to the first one while the
        // surface beside this terminal kept it.
        let mut grapheme = ffi::GhosttyTerminalModeConfig {
            mode: Mode::GRAPHEME_CLUSTER.packed(),
            value: options.grapheme_clustering,
        };
        terminal.set(
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
            (&raw mut grapheme).cast(),
        )?;

        if let Some(mut bytes) = options.scrollback_bytes {
            terminal.set(
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                (&raw mut bytes).cast(),
            )?;
        }
        if let Some(name) = &options.terminfo_name {
            let mut raw = ffi::GhosttyString { ptr: name.as_ptr(), len: name.len() };
            terminal.set(
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TERMINFO_NAME,
                (&raw mut raw).cast(),
            )?;
        }
        if let Some(mut bytes) = options.kitty_image_bytes {
            terminal.set(
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
                (&raw mut bytes).cast(),
            )?;
        }
        // Images sent as a path name a file on the daemon's machine, which a surface on
        // another machine cannot read; refused, so programs fall back to sending them inline
        // (MIP-3 section 4). Temporary files are off because the option is null.
        let mut off = false;
        terminal.set(
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_FILE,
            (&raw mut off).cast(),
        )?;
        terminal.set(
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_TEMP_FILE,
            std::ptr::null_mut(),
        )?;
        terminal.set(
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM,
            (&raw mut off).cast(),
        )?;

        if options.cell_pixels != (0, 0) {
            terminal.resize(options.columns, options.rows, options.cell_pixels)?;
        }
        Ok(terminal)
    }

    /// Ghostty's clear_screen binding, as its own Termio does it to a terminal: nothing on the
    /// alternate screen; otherwise history goes, and at a prompt (shell integration's marks) the
    /// whole screen, or elsewhere the rows above the cursor and every kitty image. True at a
    /// prompt, when Ghostty sends the shell a form feed to draw its prompt again.
    pub fn clear_screen(&mut self) -> bool {
        let mut at_prompt = false;
        // SAFETY: the terminal is live and exclusively borrowed, and the output is the type
        // muster.h documents.
        unsafe { ffi::ghostty_terminal_clear_screen(self.terminal, true, &raw mut at_prompt) };
        at_prompt
    }

    /// Forgets every kitty image on both screens, placements and all, keeping the limit: what
    /// a terminal that has only seen a replay holds, since a replay carries no images. A program
    /// that places one by id from then on is told it is not there, and sends it again.
    ///
    /// Not while an image is still arriving, which forgetting would cut off: the images stay
    /// known until the next replay forgets them.
    pub fn forget_kitty_images(&mut self) {
        let limit = self
            .get(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_IMAGE_STORAGE_LIMIT, 0u64);
        if limit == 0 || self.kitty_image_loading() {
            return;
        }
        // A limit of zero empties each screen's store, as libghostty disables images; the
        // limit put back enables them again, empty. Neither can fail for a live terminal.
        for mut bytes in [0u64, limit] {
            let _ = self.set(
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
                (&raw mut bytes).cast(),
            );
        }
    }

    /// Whether a chunked kitty image has begun arriving on either screen and not ended.
    pub fn kitty_image_loading(&self) -> bool {
        let mut loading = false;
        // SAFETY: the terminal is live for as long as self, and the out pointer is a local the
        // call writes a bool to, as muster.h documents.
        unsafe { ffi::ghostty_terminal_kitty_image_loading(self.terminal, &raw mut loading) };
        loading
    }

    fn set(
        &mut self,
        option: ffi::GhosttyTerminalOption,
        value: *mut c_void,
    ) -> Result<(), TerminalError> {
        // SAFETY: every caller pairs the option with a pointer to a local of its documented
        // input type (or null where the option documents null), and libghostty copies it.
        let result = unsafe { ffi::ghostty_terminal_set(self.terminal, option, value) };
        if result == ffi::GhosttyResult_GHOSTTY_SUCCESS {
            Ok(())
        } else {
            Err(TerminalError::CreationFailed(result))
        }
    }

    /// Who hears about the effects of what is written from now on: query answers, bells,
    /// titles, notifications. Called synchronously inside `write`, so it must not block.
    pub fn set_effect_handler(&mut self, handler: impl FnMut(Effect<'_>) + Send + 'static) {
        // SAFETY: the allocation lives as long as `self`, and `&mut self` means no callback
        // is running.
        unsafe { (*self.effects).handler = Some(Box::new(handler)) };
    }

    /// What queries are answered with from now on.
    pub fn set_answers(&mut self, answers: Answers) {
        // SAFETY: as in `set_effect_handler`.
        unsafe { (*self.effects).answers = answers };
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

    /// A new grid, and the pixel size of one cell (zero while no surface has said).
    pub fn resize(
        &mut self,
        columns: u16,
        rows: u16,
        cell_pixels: (u32, u32),
    ) -> Result<(), TerminalError> {
        let (width, height) = cell_pixels;
        // SAFETY: the handle is ours and the call takes only scalars besides.
        let result =
            unsafe { ffi::ghostty_terminal_resize(self.terminal, columns, rows, width, height) };
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

    /// Tells the terminal the foreground, background and cursor colors the app is drawing
    /// with. None leaves that color unset. As with the palette, a color a program set with
    /// OSC 10, 11 or 12 keeps the program's value.
    pub fn set_default_colors(
        &mut self,
        foreground: Option<Rgb>,
        background: Option<Rgb>,
        cursor: Option<Rgb>,
    ) {
        for (option, color) in [
            (ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, foreground),
            (ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, background),
            (ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, cursor),
        ] {
            let mut raw = color.map(Rgb::raw);
            let value =
                raw.as_mut().map_or(std::ptr::null_mut(), |raw| std::ptr::from_mut(raw).cast());
            // SAFETY: each color option reads one GhosttyColorRgb, or clears on null.
            unsafe { ffi::ghostty_terminal_set(self.terminal, option, value) };
        }
    }

    /// The most history kept, in bytes, from now on. Lowering it drops history at once.
    pub fn set_scrollback_bytes(&mut self, bytes: usize) -> Result<(), TerminalError> {
        let mut bytes = bytes;
        self.set(
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
            (&raw mut bytes).cast(),
        )
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
        // SAFETY: the handle was created by `new` and is freed exactly once, before the
        // effects its callbacks reach, which came from `Box::into_raw` and are freed once.
        unsafe {
            ffi::ghostty_terminal_free(self.terminal);
            drop(Box::from_raw(self.effects));
        }
    }
}
