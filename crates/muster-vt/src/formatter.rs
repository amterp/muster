//! A terminal's contents as text or as the VT sequences that would redraw them.
//!
//! libghostty-vt's formatter, which is what `muster pane read`, agent detection and the
//! replay all read a screen through. One call walks the whole screen inside the library,
//! rather than three FFI calls per cell.

use crate::ffi;
use crate::state::Screen;
use crate::terminal::Terminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The text, with no styling.
    Plain,
    /// The text with the SGR, hyperlinks and positioning that reproduce it.
    Vt,
}

/// Terminal state to emit around the content, in VT output only.
///
/// Every field is off by default. The formatter's own order is fixed - palette, modes and
/// tabstops before the content, the rest after - which is why the replay asks for pieces
/// rather than for everything at once (`replay.rs`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // one flag per piece of state, mirroring the C struct
pub struct Extras {
    pub palette: bool,
    pub modes: bool,
    pub scrolling_region: bool,
    pub tabstops: bool,
    pub pwd: bool,
    /// modifyOtherKeys.
    pub keyboard: bool,
    pub screen: ScreenExtras,
}

/// State that belongs to one screen rather than to the terminal: each screen has its own
/// cursor, pen and kitty keyboard flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // one flag per piece of state, mirroring the C struct
pub struct ScreenExtras {
    pub cursor: bool,
    pub style: bool,
    pub hyperlink: bool,
    pub protection: bool,
    pub kitty_keyboard: bool,
    pub charsets: bool,
}

impl ScreenExtras {
    fn raw(self) -> ffi::GhosttyFormatterScreenExtra {
        ffi::GhosttyFormatterScreenExtra {
            size: size_of::<ffi::GhosttyFormatterScreenExtra>(),
            cursor: self.cursor,
            style: self.style,
            hyperlink: self.hyperlink,
            protection: self.protection,
            kitty_keyboard: self.kitty_keyboard,
            charsets: self.charsets,
        }
    }
}

/// A span of the active area, inclusive at both ends, in reading order: from `start` to
/// the end of its row, every row between, and the start of `end`'s row through `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Column and row.
    pub start: (u16, u32),
    pub end: (u16, u32),
}

impl Selection {
    /// Whole rows of the active area, `first` through `last`.
    pub fn rows(first: u32, last: u32, columns: u16) -> Selection {
        Selection { start: (0, first), end: (columns.saturating_sub(1), last) }
    }
}

/// How to format a terminal: its active screen, with terminal-wide state around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOptions {
    pub format: Format,
    /// Join soft-wrapped rows into one line, so a reader sees a program's lines rather than
    /// the terminal's width - and a replay re-wraps them at the receiving width.
    pub unwrap: bool,
    /// Drop trailing whitespace from each non-blank row.
    pub trim: bool,
    pub extras: Extras,
    /// Only this span of the active area rather than the whole screen and its history.
    pub selection: Option<Selection>,
}

impl FormatOptions {
    pub fn plain() -> FormatOptions {
        FormatOptions {
            format: Format::Plain,
            unwrap: false,
            trim: true,
            extras: Extras::default(),
            selection: None,
        }
    }

    pub fn vt() -> FormatOptions {
        FormatOptions {
            format: Format::Vt,
            unwrap: false,
            trim: false,
            extras: Extras::default(),
            selection: None,
        }
    }

    fn raw(self) -> ffi::GhosttyFormatterTerminalOptions {
        let e = self.extras;
        ffi::GhosttyFormatterTerminalOptions {
            size: size_of::<ffi::GhosttyFormatterTerminalOptions>(),
            emit: self.format.raw(),
            unwrap: self.unwrap,
            trim: self.trim,
            extra: ffi::GhosttyFormatterTerminalExtra {
                size: size_of::<ffi::GhosttyFormatterTerminalExtra>(),
                palette: e.palette,
                modes: e.modes,
                scrolling_region: e.scrolling_region,
                tabstops: e.tabstops,
                pwd: e.pwd,
                keyboard: e.keyboard,
                screen: e.screen.raw(),
            },
            selection: std::ptr::null(),
        }
    }
}

/// How to format one screen, whether or not it is the active one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // mirrors the C options
pub struct ScreenFormatOptions {
    pub format: Format,
    pub unwrap: bool,
    pub trim: bool,
    /// Emit the screen's rows. Without them, only the extras.
    pub content: bool,
    /// Emit the blank rows below the last row with text as well, so the output spans every
    /// row of the screen and a replay of it lines up with the original.
    pub trailing_blank_rows: bool,
    pub extras: ScreenExtras,
}

impl ScreenFormatOptions {
    fn raw(self) -> ffi::GhosttyFormatterScreenOptions {
        ffi::GhosttyFormatterScreenOptions {
            size: size_of::<ffi::GhosttyFormatterScreenOptions>(),
            emit: self.format.raw(),
            unwrap: self.unwrap,
            trim: self.trim,
            content: self.content,
            trailing_blank_rows: self.trailing_blank_rows,
            extra: self.extras.raw(),
        }
    }
}

impl Format {
    fn raw(self) -> ffi::GhosttyFormatterFormat {
        match self {
            Format::Plain => ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN,
            Format::Vt => ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_VT,
        }
    }
}

impl Terminal {
    /// The active screen, history included, formatted as `options` asks.
    pub fn format(&self, options: FormatOptions) -> Vec<u8> {
        let mut raw = options.raw();
        let selection;
        if let Some(span) = options.selection {
            let (Some(start), Some(end)) = (
                self.grid_ref(
                    ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_ACTIVE,
                    span.start.0,
                    span.start.1,
                ),
                self.grid_ref(
                    ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_ACTIVE,
                    span.end.0,
                    span.end.1,
                ),
            ) else {
                return Vec::new();
            };
            selection = ffi::GhosttySelection {
                size: size_of::<ffi::GhosttySelection>(),
                start,
                end,
                rectangle: false,
            };
            raw.selection = &raw const selection;
        }

        let mut formatter: ffi::GhosttyFormatter = std::ptr::null_mut();
        // SAFETY: the options are fully initialized with their `size` fields set; the
        // selection, if any, outlives the call that copies it; and the formatter borrows the
        // terminal only until `run` frees it, inside this borrow - so the grid refs, which
        // are valid until the terminal next changes, are too.
        let created = unsafe {
            ffi::ghostty_formatter_terminal_new(
                std::ptr::null(),
                &raw mut formatter,
                self.handle(),
                raw,
            )
        };
        run(created, formatter)
    }

    /// Rows `first` through `last` of the active area as plain text, one line per row, in
    /// one call.
    ///
    /// What agent detection and `muster pane read` read a screen through: `viewport` costs
    /// several FFI calls per cell, and detection reads the bottom of every pane every few
    /// hundred milliseconds. Blank rows at the end of the range are left out, as the
    /// formatter leaves them out everywhere.
    pub fn text(&self, first: u16, last: u16) -> String {
        let columns = self.columns();
        let options = FormatOptions {
            selection: Some(Selection::rows(u32::from(first), u32::from(last), columns)),
            ..FormatOptions::plain()
        };
        String::from_utf8_lossy(&self.format(options)).into_owned()
    }

    /// The extras `options` asks for, with no screen content at all: the terminal's state
    /// on its own, for a replay that formats each screen separately.
    ///
    /// Through a patch Muster carries on libghostty (`deps/ghostty-patches/`).
    pub fn format_state(&self, options: FormatOptions) -> Vec<u8> {
        let mut formatter: ffi::GhosttyFormatter = std::ptr::null_mut();
        // SAFETY: as in `format`; the selection is null, which this constructor requires.
        let created = unsafe {
            ffi::ghostty_formatter_terminal_state_new(
                std::ptr::null(),
                &raw mut formatter,
                self.handle(),
                options.raw(),
            )
        };
        run(created, formatter)
    }

    /// One screen, active or not - the primary screen's history while a program holds the
    /// alternate one is the case that needs it.
    ///
    /// Through a patch Muster carries on libghostty (`deps/ghostty-patches/`).
    pub fn format_screen(&self, screen: Screen, options: ScreenFormatOptions) -> Vec<u8> {
        let mut formatter: ffi::GhosttyFormatter = std::ptr::null_mut();
        // SAFETY: as in `format`.
        let created = unsafe {
            ffi::ghostty_formatter_screen_new(
                std::ptr::null(),
                &raw mut formatter,
                self.handle(),
                screen.raw(),
                options.raw(),
            )
        };
        run(created, formatter)
    }
}

/// Formats once and frees the formatter. A formatter that could not be created, or that
/// could not allocate its output, yields nothing.
fn run(created: ffi::GhosttyResult, formatter: ffi::GhosttyFormatter) -> Vec<u8> {
    if created != ffi::GhosttyResult_GHOSTTY_SUCCESS || formatter.is_null() {
        return Vec::new();
    }

    let mut pointer: *mut u8 = std::ptr::null_mut();
    let mut length = 0usize;
    // SAFETY: the formatter was just created; the out parameters are ours.
    let result = unsafe {
        ffi::ghostty_formatter_format_alloc(
            formatter,
            std::ptr::null(),
            &raw mut pointer,
            &raw mut length,
        )
    };
    let bytes = if result == ffi::GhosttyResult_GHOSTTY_SUCCESS && !pointer.is_null() {
        // SAFETY: libghostty allocated `length` bytes at `pointer` for us.
        let bytes = unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec();
        // SAFETY: freed once, with the default allocator it was allocated with.
        unsafe { ffi::ghostty_free(std::ptr::null(), pointer, length) };
        bytes
    } else {
        Vec::new()
    };
    // SAFETY: created by the caller and freed exactly once, here.
    unsafe { ffi::ghostty_formatter_free(formatter) };
    bytes
}
