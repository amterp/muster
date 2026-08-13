//! What a terminal screen holds, as data, and as text a reviewer can read in a diff.
//!
//! Deliberately only text and cell widths. Colors and attributes are real, and a snapshot
//! that carried them would be a better oracle - but it would also be a wall of noise in
//! every diff, and `docs/testing.md` wants cases a reviewer can read. Styling gets added
//! when a test needs it to fail honestly.

use std::fmt::Write as _;

use crate::ffi;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The whole grapheme cluster in this cell. Empty for an unwritten cell.
    pub text: String,
    pub width: Width,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<Cell>,
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
