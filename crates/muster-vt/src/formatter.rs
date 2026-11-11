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
/// Every field is off by default, and every formatter is told so explicitly: libghostty's own
/// default adds the palette, and a replay that restated the palette would pin the receiver to
/// this terminal's colors rather than its own theme. The formatter's own order is fixed -
/// palette, modes and tabstops before the content, the rest after - which is why the replay
/// asks for pieces rather than for everything at once (`replay.rs`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // one flag per piece of state, mirroring the C bits
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
#[allow(clippy::struct_excessive_bools)] // one flag per piece of state, mirroring the C bits
pub struct ScreenExtras {
    pub cursor: bool,
    pub style: bool,
    pub hyperlink: bool,
    pub protection: bool,
    pub kitty_keyboard: bool,
    pub charsets: bool,
}

impl Extras {
    /// As `GHOSTTY_FORMATTER_EXTRA_*` bits.
    pub(crate) fn bits(self) -> ffi::GhosttyFormatterExtras {
        [
            (self.palette, ffi::GHOSTTY_FORMATTER_EXTRA_PALETTE),
            (self.modes, ffi::GHOSTTY_FORMATTER_EXTRA_MODES),
            (self.scrolling_region, ffi::GHOSTTY_FORMATTER_EXTRA_SCROLLING_REGION),
            (self.tabstops, ffi::GHOSTTY_FORMATTER_EXTRA_TABSTOPS),
            (self.pwd, ffi::GHOSTTY_FORMATTER_EXTRA_PWD),
            (self.keyboard, ffi::GHOSTTY_FORMATTER_EXTRA_KEYBOARD),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(self.screen.bits(), |bits, (_, bit)| bits | bit)
    }
}

impl ScreenExtras {
    fn bits(self) -> ffi::GhosttyFormatterExtras {
        [
            (self.cursor, ffi::GHOSTTY_FORMATTER_EXTRA_CURSOR),
            (self.style, ffi::GHOSTTY_FORMATTER_EXTRA_STYLE),
            (self.hyperlink, ffi::GHOSTTY_FORMATTER_EXTRA_HYPERLINK),
            (self.protection, ffi::GHOSTTY_FORMATTER_EXTRA_PROTECTION),
            (self.kitty_keyboard, ffi::GHOSTTY_FORMATTER_EXTRA_KITTY_KEYBOARD),
            (self.charsets, ffi::GHOSTTY_FORMATTER_EXTRA_CHARSETS),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(0, |bits, (_, bit)| bits | bit)
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
    /// Drop trailing whitespace from each non-blank row. In VT output, leaving it off also
    /// keeps the blank rows below the last row with text, so the output spans the screen.
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
}

/// Which rows of a screen to format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rows {
    /// History and the active area.
    All,
    /// The active area only, which is how a surface that fell behind is caught up without
    /// resending what it already holds.
    Active,
    /// History only, ending at the row above the active area.
    History,
}

impl Rows {
    fn raw(self) -> std::ffi::c_int {
        let raw = match self {
            Rows::All => ffi::GHOSTTY_MUSTER_ROWS_ALL,
            Rows::Active => ffi::GHOSTTY_MUSTER_ROWS_ACTIVE,
            Rows::History => ffi::GHOSTTY_MUSTER_ROWS_HISTORY,
        };
        std::ffi::c_int::try_from(raw).expect("the GHOSTTY_MUSTER_ROWS_* values are small")
    }
}

/// How to format one screen, whether or not it is the active one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenFormatOptions {
    pub format: Format,
    pub unwrap: bool,
    /// As `FormatOptions::trim`: off in VT output keeps the blank rows at the bottom, so a
    /// replay of the screen lines up with the original.
    pub trim: bool,
    pub rows: Rows,
    pub extras: ScreenExtras,
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
        self.format_in(ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_ACTIVE, options)
    }

    /// As `format`, with the selection's rows counted in `tag`'s coordinates.
    fn format_in(&self, tag: ffi::GhosttyPointTag, options: FormatOptions) -> Vec<u8> {
        let selection = match options.selection {
            Some(span) => {
                let (Some(start), Some(end)) = (
                    self.grid_ref(tag, span.start.0, span.start.1),
                    self.grid_ref(tag, span.end.0, span.end.1),
                ) else {
                    return Vec::new();
                };
                Some(ffi::GhosttySelection { start, end, rectangle: false })
            }
            None => None,
        };

        let mut formatter: ffi::GhosttyFormatter = std::ptr::null_mut();
        // SAFETY: the formatter borrows the terminal only until it is freed below, inside this
        // borrow.
        let created = unsafe {
            ffi::ghostty_formatter_terminal_new(std::ptr::null(), &raw mut formatter, self.handle())
        };
        if created != ffi::GhosttyResult_GHOSTTY_SUCCESS || formatter.is_null() {
            return Vec::new();
        }
        let formatter = Formatter(formatter);

        let emit = options.format.raw();
        let extras = options.extras.bits();
        let set = formatter.set(ffi::GhosttyFormatterOption_GHOSTTY_FORMATTER_OPT_EMIT, &emit)
            && formatter.set(ffi::GhosttyFormatterOption_GHOSTTY_FORMATTER_OPT_UNWRAP, &options.unwrap)
            && formatter.set(ffi::GhosttyFormatterOption_GHOSTTY_FORMATTER_OPT_TRIM, &options.trim)
            && formatter.set(ffi::GhosttyFormatterOption_GHOSTTY_FORMATTER_OPT_EXTRAS, &extras)
            && selection.as_ref().is_none_or(|selection| {
                formatter.set(ffi::GhosttyFormatterOption_GHOSTTY_FORMATTER_OPT_SELECTION, selection)
            });
        if !set {
            return Vec::new();
        }

        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut length = 0usize;
        // SAFETY: the formatter is live; the selection it copied is valid until the terminal
        // next changes, which cannot happen inside this borrow; the out parameters are ours.
        let result = unsafe {
            ffi::ghostty_formatter_format_alloc(
                formatter.0,
                std::ptr::null(),
                &raw mut pointer,
                &raw mut length,
            )
        };
        take(result, pointer, length)
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

    /// Rows `first` through `last` of the active screen as plain text, counted from the oldest
    /// row of history it still holds rather than from the top of the active area.
    ///
    /// What `muster pane read` pages through. A range past the last row reads as nothing.
    pub fn screen_text(&self, first: u32, last: u32) -> String {
        let last = last.min(u32::try_from(self.total_rows()).unwrap_or(u32::MAX).saturating_sub(1));
        if first > last {
            return String::new();
        }
        let options = FormatOptions {
            selection: Some(Selection::rows(first, last, self.columns())),
            ..FormatOptions::plain()
        };
        let text = self.format_in(ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN, options);
        String::from_utf8_lossy(&text).into_owned()
    }

    /// The `extras` with no screen content at all, as VT: the terminal's state on its own,
    /// for a replay that formats each screen separately. The cursor, when asked for, is placed
    /// for origin mode and the right margin as the receiver will have them once the modes and
    /// the scrolling region are restated.
    ///
    /// Through a patch Muster carries on libghostty (`deps/ghostty-patches/`).
    pub fn format_state(&self, extras: Extras) -> Vec<u8> {
        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut length = 0usize;
        // SAFETY: the terminal is live for this borrow; the out parameters are ours.
        let result = unsafe {
            ffi::ghostty_terminal_format_state(
                self.handle(),
                extras.bits(),
                std::ptr::null(),
                &raw mut pointer,
                &raw mut length,
            )
        };
        take(result, pointer, length)
    }

    /// One screen, active or not - the primary screen's history while a program holds the
    /// alternate one is the case that needs it.
    ///
    /// Through a patch Muster carries on libghostty (`deps/ghostty-patches/`).
    pub fn format_screen(&self, screen: Screen, options: ScreenFormatOptions) -> Vec<u8> {
        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut length = 0usize;
        // SAFETY: the terminal is live for this borrow; the out parameters are ours.
        let result = unsafe {
            ffi::ghostty_terminal_format_screen(
                self.handle(),
                screen.raw(),
                options.rows.raw(),
                options.format.raw(),
                options.unwrap,
                options.trim,
                options.extras.bits(),
                std::ptr::null(),
                &raw mut pointer,
                &raw mut length,
            )
        };
        take(result, pointer, length)
    }
}

/// A formatter, freed when it goes out of scope.
struct Formatter(ffi::GhosttyFormatter);

impl Formatter {
    /// Sets one option, whose value has the type the header lists for it.
    fn set<T>(&self, option: ffi::GhosttyFormatterOption, value: &T) -> bool {
        // SAFETY: the formatter is live, and copies the value before returning.
        let result = unsafe {
            ffi::ghostty_formatter_set(self.0, option, std::ptr::from_ref(value).cast())
        };
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS
    }
}

impl Drop for Formatter {
    fn drop(&mut self) {
        // SAFETY: created by ghostty_formatter_terminal_new and freed exactly once, here.
        unsafe { ffi::ghostty_formatter_free(self.0) };
    }
}

/// The bytes libghostty allocated for us, copied out and freed. A failed format, or one that
/// could not allocate its output, yields nothing.
fn take(result: ffi::GhosttyResult, pointer: *mut u8, length: usize) -> Vec<u8> {
    if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || pointer.is_null() {
        return Vec::new();
    }
    // SAFETY: libghostty allocated `length` bytes at `pointer` for us.
    let bytes = unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec();
    // SAFETY: freed once, with the default allocator it was allocated with.
    unsafe { ffi::ghostty_free(std::ptr::null(), pointer, length) };
    bytes
}
