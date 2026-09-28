//! What a terminal screen holds, as data, and as text a reviewer can read in a diff.
//!
//! A cell carries its style, and a row whether it soft-wraps, because the replay oracle
//! needed both to fail honestly: a replay that drops a status bar's background, or turns a
//! soft wrap into a hard one, draws the same text. The rendered snapshot stays text and
//! widths only - colors in every snapshot diff would be a wall of noise, and
//! `docs/testing.md` wants cases a reviewer can read.

use std::fmt::Write as _;

use crate::ffi;
use crate::state::Rgb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Narrow,
    Wide,
    /// The second half of a wide character. Holds no text of its own.
    SpacerTail,
    /// Padding before a wide character that would not fit at the end of a line.
    SpacerHead,
}

impl Width {
    pub(crate) fn from_raw(raw: ffi::GhosttyCellWide) -> Width {
        match raw {
            ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_WIDE => Width::Wide,
            ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL => Width::SpacerTail,
            ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_HEAD => Width::SpacerHead,
            _ => Width::Narrow,
        }
    }
}

/// A color as a cell refers to it: by palette index, so it follows the palette, or direct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    Palette(u8),
    Rgb(Rgb),
}

/// What SGR left on a cell.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // one flag per SGR attribute
pub struct Style {
    pub foreground: Option<Color>,
    pub background: Option<Color>,
    pub underline_color: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    /// libghostty's underline kind: none, single, double, curly, dotted, dashed.
    pub underline: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The whole grapheme cluster in this cell. Empty for an unwritten cell.
    pub text: String,
    pub width: Width,
    /// Includes a background an erase left with no text in the cell.
    pub style: Style,
    /// Set by DECSCA, so a selective erase leaves it alone.
    pub protected: bool,
    /// The OSC 8 link this cell belongs to.
    pub hyperlink: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// Whether the line continues onto the next row because it ran out of width, rather
    /// than because the program moved to a new line.
    pub wraps: bool,
}

impl Row {
    /// The row as a user would read it.
    ///
    /// Spacer tails are dropped rather than rendered as blanks: the wide character ahead of
    /// them already occupies two columns on any terminal showing this text, so emitting
    /// both would widen every CJK line in the snapshot by its own length.
    pub fn text(&self) -> String {
        self.cells
            .iter()
            .filter(|cell| cell.width != Width::SpacerTail)
            .map(|cell| if cell.text.is_empty() { " " } else { &cell.text })
            .collect()
    }

    /// [`Row::text`] as far as someone typed it: a cell drawn faint - a suggestion, a hint,
    /// which programs draw faint because nobody typed them - reads as blank, and so does an
    /// inverse cell just before a faint one, the caret a program draws over a suggestion's
    /// first letter. Blanked a character for a character, so a column found in `text` is the
    /// same column here.
    pub fn typed_text(&self) -> String {
        let cells: Vec<&Cell> =
            self.cells.iter().filter(|cell| cell.width != Width::SpacerTail).collect();
        let mut typed = String::new();
        for (at, cell) in cells.iter().enumerate() {
            let caret_over_faint =
                cell.style.inverse && cells.get(at + 1).is_some_and(|next| next.style.faint);
            let text = if cell.text.is_empty() { " " } else { cell.text.as_str() };
            if cell.style.faint || caret_over_faint {
                typed.extend(std::iter::repeat_n(' ', text.chars().count()));
            } else {
                typed.push_str(text);
            }
        }
        typed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub column: u16,
    pub row: u16,
    pub is_visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    pub rows: Vec<Row>,
    pub cursor: Cursor,
}

impl Grid {
    /// The grid as a snapshot file holds it.
    ///
    /// Two properties matter more than looking nice. Trailing blanks are cut from every
    /// row, because a grid is mostly empty space and a file carrying 80 columns of trailing
    /// whitespace per line is one save-with-trim away from a spurious diff - which would
    /// train a reviewer to ignore snapshot changes, the one habit that makes the whole
    /// approach worthless. And row numbers are on every line, because without them a diff
    /// of a mostly-blank screen shows two identical-looking hunks and no way to tell which
    /// row moved.
    pub fn render(&self) -> String {
        let columns = self.rows.first().map_or(0, |row| row.cells.len());
        let width = self.rows.len().to_string().len();

        let hidden = if self.cursor.is_visible { "" } else { " (hidden)" };
        let mut out = format!(
            "grid {columns}x{}\ncursor {},{}{hidden}\n\n",
            self.rows.len(),
            self.cursor.column,
            self.cursor.row,
        );

        for (index, row) in self.rows.iter().enumerate() {
            let text = row.text();
            let text = text.trim_end_matches(' ');
            // A separator even on empty rows, so a row that gained a single leading space
            // shows up as a changed line rather than as an invisible one.
            if text.is_empty() {
                let _ = writeln!(out, "{index:>width$} |");
            } else {
                let _ = writeln!(out, "{index:>width$} | {text}");
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[(&str, Style)]) -> Row {
        let cells = cells
            .iter()
            .map(|(text, style)| Cell {
                text: (*text).to_string(),
                width: Width::Narrow,
                style: style.clone(),
                protected: false,
                hyperlink: None,
            })
            .collect();
        Row { cells, wraps: false }
    }

    fn plain() -> Style {
        Style::default()
    }

    fn faint() -> Style {
        Style { faint: true, ..Style::default() }
    }

    fn inverse() -> Style {
        Style { inverse: true, ..Style::default() }
    }

    #[test]
    fn a_suggestion_drawn_faint_is_not_typed() {
        let row = row(&[("❯", plain()), (" ", plain()), ("T", faint()), ("r", faint())]);
        assert_eq!(row.text(), "❯ Tr");
        assert_eq!(row.typed_text(), "❯   ");
    }

    #[test]
    fn a_caret_over_a_suggestion_is_part_of_it() {
        let row = row(&[("❯", plain()), (" ", plain()), ("T", inverse()), ("r", faint())]);
        assert_eq!(row.typed_text(), "❯   ");
    }

    #[test]
    fn typed_text_keeps_its_caret_and_leaves_out_what_follows_faint() {
        let row = row(&[
            ("h", plain()),
            ("i", plain()),
            (" ", inverse()),
            ("m", faint()),
            ("x", plain()),
            ("y", inverse()),
        ]);
        // A caret before faint text blanks, so "hi" stays typed; "x" and a caret on "y" do too.
        assert_eq!(row.typed_text(), "hi  xy");
    }
}
