//! A terminal's contents as text or as the VT sequences that would redraw them.
//!
//! libghostty-vt's formatter, which is what `muster pane read`, agent detection and the
//! replay all read a screen through. One call walks the whole screen inside the library,
//! rather than three FFI calls per cell.

use crate::ffi;
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
    pub keyboard: bool,
    pub cursor: bool,
    pub style: bool,
    pub hyperlink: bool,
    pub protection: bool,
    pub kitty_keyboard: bool,
    pub charsets: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOptions {
    pub format: Format,
    /// Join soft-wrapped rows into one line, so a reader sees a program's lines rather than
    /// the terminal's width - and a replay re-wraps them at the receiving width.
    pub unwrap: bool,
    /// Drop trailing whitespace from each non-blank row.
    pub trim: bool,
    pub extras: Extras,
}

impl FormatOptions {
    pub fn plain() -> FormatOptions {
        FormatOptions {
            format: Format::Plain,
            unwrap: false,
            trim: true,
            extras: Extras::default(),
        }
    }

    pub fn vt() -> FormatOptions {
        FormatOptions { format: Format::Vt, unwrap: false, trim: false, extras: Extras::default() }
    }

    fn raw(self) -> ffi::GhosttyFormatterTerminalOptions {
        let e = self.extras;
        ffi::GhosttyFormatterTerminalOptions {
            size: size_of::<ffi::GhosttyFormatterTerminalOptions>(),
            emit: match self.format {
                Format::Plain => ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN,
                Format::Vt => ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_VT,
            },
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
                screen: ffi::GhosttyFormatterScreenExtra {
                    size: size_of::<ffi::GhosttyFormatterScreenExtra>(),
                    cursor: e.cursor,
                    style: e.style,
                    hyperlink: e.hyperlink,
                    protection: e.protection,
                    kitty_keyboard: e.kitty_keyboard,
                    charsets: e.charsets,
                },
            },
            selection: std::ptr::null(),
        }
    }
}

impl Terminal {
    /// The active screen, history included, formatted as `options` asks.
    pub fn format(&self, options: FormatOptions) -> Vec<u8> {
        let mut formatter: ffi::GhosttyFormatter = std::ptr::null_mut();
        // SAFETY: the options are fully initialized with their `size` fields set, and the
        // formatter borrows the terminal only until it is freed below, inside this borrow.
        let created = unsafe {
            ffi::ghostty_formatter_terminal_new(
                std::ptr::null(),
                &raw mut formatter,
                self.handle(),
                options.raw(),
            )
        };
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
        // SAFETY: created above and freed exactly once.
        unsafe { ffi::ghostty_formatter_free(formatter) };
        bytes
    }
}
