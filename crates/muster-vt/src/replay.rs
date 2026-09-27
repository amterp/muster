//! The bytes that bring a fresh terminal to this one's state.
//!
//! A surface attaching to a pane, and a daemon taking a pane over from another, both start
//! from a terminal that has seen none of the pane's output, and the replay is what they parse
//! first. MIP-3 section 5 gives the order. This is Muster's composition of it from the
//! formatter's pieces, because the formatter's own all-in-one output replays content in
//! whatever insert and wrap modes the program left behind, leaves the cursor wherever its
//! tabstops put it, and reaches only the active screen
//! (`docs/observations/libghostty-9f9b8d1d.md`, section 11).
//!
//! The judge is `tests/replay.rs`: a terminal fed a replay must be indistinguishable from the
//! one it came from, before and after both receive the same further bytes.

use crate::formatter::{
    Extras, Format, FormatOptions, ScreenExtras, ScreenFormatOptions, Selection,
};
use crate::grid::Width;
use crate::modes::Mode;
use crate::state::{Rgb, Screen};
use crate::terminal::Terminal;

const RESET: &[u8] = b"\x1bc";
const HOME: &[u8] = b"\x1b[H";

/// What RIS would reset, short of the history it would erase: the primary screen, a pen,
/// hyperlink, protection and charsets at their defaults (an erase paints with the pen's
/// background), margins and a scrolling region spanning the screen, no frozen rendering, no
/// kitty flags or modifyOtherKeys, and every color a program set taken back. Then the screen
/// erased, from the top. The modes the content is written under are stated here too; every
/// mode is stated again after it.
const CATCH_UP_RESET: &[u8] = b"\x1b[?2026l\x1b[?1049l\x1b[?1047l\x1b[?47l\
    \x1b[0m\x1b]8;;\x1b\\\x1b[0\"q\x1b(B\x1b)B\x1b*B\x1b+B\x0f\
    \x1b[4l\x1b[?7h\x1b[?6l\x1b[?69l\x1b[r\x1b[=0;1u\x1b[>4m\
    \x1b]104\x1b\\\x1b]110\x1b\\\x1b]111\x1b\\\x1b]112\x1b\\\
    \x1b[H\x1b[2J";

/// The same for the alternate screen once it is entered, where the kitty flags are its own.
const ALTERNATE_RESET: &[u8] = b"\x1b[2J\x1b[=0;1u";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Into a fresh terminal.
    Replay,
    /// Into a terminal that fell behind, keeping its history.
    CatchUp,
}

/// A screen's pen: what the next character printed there will look like.
const PEN: ScreenExtras = ScreenExtras {
    cursor: false,
    style: true,
    hyperlink: true,
    protection: true,
    kitty_keyboard: true,
    charsets: true,
};

impl Terminal {
    pub fn replay(&self) -> Vec<u8> {
        let mut out = RESET.to_vec();
        self.compose(&mut out, Kind::Replay);
        out
    }

    /// The bytes that bring a terminal which fell behind this one to its screen, keeping the
    /// history it already holds.
    ///
    /// A surface that stopped receiving a pane's output mid-flood holds a stale screen and
    /// whatever state the program left when the bytes stopped reaching it. A replay would
    /// resend the whole history, which over ssh is the flood again. This resends the screen
    /// and states every piece of state instead: RIS would erase the receiver's history, so
    /// what RIS resets is reset piece by piece, before the replay's own steps.
    pub fn catch_up(&self) -> Vec<u8> {
        let mut out = CATCH_UP_RESET.to_vec();
        self.compose(&mut out, Kind::CatchUp);
        out
    }

    fn compose(&self, out: &mut Vec<u8>, kind: Kind) {
        let alternate = self.active_screen() == Screen::Alternate;
        let history = kind == Kind::Replay;

        // Before any content: whether a ZWJ sequence occupies one cell decides where every
        // later cell lands.
        out.extend(sequence(Mode::GRAPHEME_CLUSTER, self.mode(Mode::GRAPHEME_CLUSTER)));

        // The primary screen always, so a program leaving the alternate screen finds what was
        // there before it. While the alternate one is active the primary's cursor and pen are
        // what entering it saved, and leaving it restores them, so they are set before the
        // switch.
        let primary_extras =
            if alternate { ScreenExtras { cursor: true, ..PEN } } else { ScreenExtras::default() };
        out.extend(self.format_screen(Screen::Primary, content(primary_extras, history)));
        if alternate {
            out.extend(sequence(self.alternate_entry(), true));
            // The alternate screen starts with the cursor where the primary left it.
            out.extend_from_slice(HOME);
            if kind == Kind::CatchUp {
                // Entering it with 47 or 1047 keeps whatever the receiver last drew there.
                out.extend_from_slice(ALTERNATE_RESET);
            }
            out.extend(
                self.format_screen(Screen::Alternate, content(ScreenExtras::default(), false)),
            );
        }

        // After the content, which must be written with wrapping on and insert mode off
        // whatever the program left them as. Origin mode waits: setting it moves the cursor
        // into the scrolling region, which does not exist yet.
        self.modes(out);
        let movers = self.format_state(FormatOptions {
            extras: Extras {
                tabstops: true,
                scrolling_region: true,
                keyboard: true,
                ..Extras::default()
            },
            ..FormatOptions::vt()
        });
        let region = Region::parse(&movers);
        out.extend(movers);
        let origin = self.mode(Mode::ORIGIN);
        if origin {
            out.extend(sequence(Mode::ORIGIN, true));
        }

        self.colors(out);
        self.identity(out, kind);
        self.cursor_position(out, origin.then_some(region));

        // Last, because re-drawing a pending wrap prints with the cell's own style.
        out.extend(self.format_state(FormatOptions {
            extras: Extras { screen: PEN, ..Extras::default() },
            ..FormatOptions::vt()
        }));
    }

    /// The screen-switch mode the program entered the alternate screen with, so leaving it
    /// the same way restores (or does not restore) the cursor as it would have.
    fn alternate_entry(&self) -> Mode {
        [Mode::ALT_SCREEN_SAVE, Mode::ALT_SCREEN, Mode::ALT_SCREEN_LEGACY]
            .into_iter()
            .find(|&mode| self.mode(mode))
            .unwrap_or(Mode::ALT_SCREEN_SAVE)
    }

    /// Every mode's value, stated outright.
    ///
    /// Stated rather than diffed against defaults, because the defaults that matter are the
    /// receiver's, which a replay cannot see: a surface configures its own reset defaults.
    /// Unsets go before sets, because the mouse modes share one field and an unset clears
    /// whichever of them is on.
    fn modes(&self, out: &mut Vec<u8>) {
        let modes: Vec<(Mode, bool)> = Mode::all()
            .filter(|&mode| replayable(mode))
            .map(|mode| (mode, self.mode(mode)))
            .collect();
        for enabled in [false, true] {
            for &(mode, value) in &modes {
                if value == enabled {
                    out.extend(sequence(mode, value));
                }
            }
        }
    }

    /// Only what a program changed, so a replayed pane still follows the app's theme.
    fn colors(&self, out: &mut Vec<u8>) {
        let current = self.palette();
        let default = self.default_palette();
        for (index, (now, was)) in current.iter().zip(default.iter()).enumerate() {
            if now != was {
                out.extend(format!("\x1b]4;{index};{}\x1b\\", spec(*now)).into_bytes());
            }
        }
        for (code, now, was) in [
            (10, self.foreground(), self.default_foreground()),
            (11, self.background(), self.default_background()),
            (12, self.cursor_color(), self.default_cursor_color()),
        ] {
            if let Some(now) = now.filter(|&now| Some(now) != was) {
                out.extend(format!("\x1b]{code};{}\x1b\\", spec(now)).into_bytes());
            }
        }
    }

    /// The title and directory the program reported, which a daemon rebuilt from a replay
    /// reports in turn. A catch-up states them even when empty, because the receiver may
    /// still hold ones the program has since cleared.
    fn identity(&self, out: &mut Vec<u8>, kind: Kind) {
        let stated = |value: &str| kind == Kind::CatchUp || !value.is_empty();
        let title = self.title();
        if stated(&title) {
            out.extend(format!("\x1b]2;{title}\x1b\\").into_bytes());
        }
        let pwd = self.pwd();
        if stated(&pwd) {
            out.extend(format!("\x1b]7;{pwd}\x1b\\").into_bytes());
        }
    }

    /// The cursor, where the program left it, in the coordinates the receiver will read the
    /// position in.
    ///
    /// A cursor waiting to wrap at the last column cannot be put there by any position
    /// sequence, which always clears the wait. Printing the cell that is already there does
    /// put it there, and leaves the cell as it was.
    fn cursor_position(&self, out: &mut Vec<u8>, origin: Option<Region>) {
        let cursor = self.cursor();
        let waiting = self.pending_wrap();
        // A wide character ending in the last column leaves the cursor on its second half.
        // Re-printed from there it would not fit, and would wrap for real; from its first
        // half it lands where it was and leaves the same wait.
        let column =
            if waiting && self.active_width(cursor.column, cursor.row) == Some(Width::SpacerTail) {
                cursor.column.saturating_sub(1)
            } else {
                cursor.column
            };

        let (top, left) = origin.map_or((0, 0), |region| (region.top, region.left));
        let row = cursor.row.saturating_sub(top) + 1;
        let relative_column = column.saturating_sub(left) + 1;
        out.extend(format!("\x1b[{row};{relative_column}H").into_bytes());

        if waiting {
            let cell = Selection {
                start: (column, u32::from(cursor.row)),
                end: (column, u32::from(cursor.row)),
            };
            out.extend(self.format(FormatOptions { selection: Some(cell), ..FormatOptions::vt() }));
        }
    }
}

/// Screen content that lines up with the original: soft wraps kept as wraps, so the receiver
/// re-wraps them at the same width, and every row down to the bottom, so its history holds
/// the same number of rows and its cursor lands on the same one.
fn content(extras: ScreenExtras, history: bool) -> ScreenFormatOptions {
    ScreenFormatOptions {
        format: Format::Vt,
        unwrap: true,
        trim: false,
        content: true,
        trailing_blank_rows: true,
        history,
        extras,
    }
}

/// Whether stating this mode in a replay reproduces it, rather than doing something else.
fn replayable(mode: Mode) -> bool {
    ![
        // The screen is entered once, before its content; entering again clears it.
        Mode::ALT_SCREEN_LEGACY,
        Mode::ALT_SCREEN,
        Mode::ALT_SCREEN_SAVE,
        // Setting it saves the cursor and unsetting restores it: an action, not a state.
        Mode::SAVE_CURSOR,
        // DECCOLM resizes and clears the screen it is set on.
        Mode::COLUMN_132,
        // Would hold the receiver's rendering until the program ends the update, which a
        // program mid-update does on its own and a replay has no business doing for it.
        Mode::SYNC_OUTPUT,
        // Set separately, after the scrolling region it makes positions relative to.
        Mode::ORIGIN,
    ]
    .contains(&mode)
}

fn sequence(mode: Mode, enabled: bool) -> Vec<u8> {
    mode.sequence(enabled).into_bytes()
}

fn spec(color: Rgb) -> String {
    format!("rgb:{:02x}/{:02x}/{:02x}", color.r, color.g, color.b)
}

/// The scrolling region's top-left corner, zero-based, which origin mode measures positions
/// from.
///
/// Read back out of the formatter's own DECSTBM and DECSLRM rather than from a terminal
/// query, because libghostty has no query for it, and a patch adding one would sit in
/// `c/terminal.zig` - 53 upstream commits in the three months to 2026-09-26, against 4 for
/// the file the carried patch touches - where a re-pin is far likelier to conflict. The
/// formatter writes both only when they differ from the whole screen (the terminal
/// formatter's scrolling-region extra in `formatter.zig`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Region {
    top: u16,
    left: u16,
}

impl Region {
    fn parse(movers: &[u8]) -> Region {
        let text = String::from_utf8_lossy(movers);
        // The first parameter of the `CSI <n> ; <n> <final>` sequence, one-based.
        let first = |final_byte: char| -> Option<u16> {
            text.split("\x1b[").find_map(|sequence| {
                let parameters_end = sequence.find(|c: char| !c.is_ascii_digit() && c != ';')?;
                if !sequence[parameters_end..].starts_with(final_byte) {
                    return None;
                }
                let (start, _) = sequence[..parameters_end].split_once(';')?;
                start.parse::<u16>().ok()
            })
        };
        Region {
            top: first('r').map_or(0, |one_based| one_based.saturating_sub(1)),
            left: first('s').map_or(0, |one_based| one_based.saturating_sub(1)),
        }
    }
}
