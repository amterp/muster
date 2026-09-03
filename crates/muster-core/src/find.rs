//! Looking for text in a pane whose history is somewhere else.
//!
//! A pane's scrollback belongs to the daemon. The surface showing it is repainted from a
//! frame stream and keeps no history, so the renderer's own find - which libghostty has,
//! and which covers a full scrollback for a terminal it owns - would search one screen
//! here and answer "no results" for text three pages up
//! (`observations/libghostty-9f9b8d1d.md` section 10). The history is the daemon's, and
//! the daemon cannot search it either.
//!
//! So Muster reads and matches, and the whole of it is `found_in` below: a pure function
//! over the text a backend handed back. Asking a backend is one method on
//! `BackendChannel`, so a daemon that grows its own search replaces that method's body
//! and nothing here moves.
//!
//! **The reach is honest rather than complete**, and [`Reach`] is how. herdr answers at
//! most a thousand rows and offers no way to page past them
//! (`observations/herdr-0.8.0.md` section 17), so a long pane is searched in part. A pane
//! showing a full-screen program is worse: it holds no history behind its screen at all,
//! and a read of it comes back looking exactly like a complete one (section 17, "a pane on
//! the alternate screen"). A confident "no results" over four fifths of a pane, or over a
//! pane whose history nobody has, is worse than no find at all - so the answer says which
//! of the three it is rather than leaving a bar to guess.
//!
//! **A row here is a row of the pane, not a row of the read.** herdr trims the blank
//! remainder of the viewport off the bottom of what it hands back, so a read's last row is
//! the pane's last printed row rather than its bottom row. [`found_in`] adds those trimmed
//! rows back, because everything downstream treats a hit's row as the offset to scroll a
//! pane by - and a hit that is 19 rows short of where it is scrolls to blank space.

use std::ops::Range;

use crate::mirror::backend::Viewport;

/// What somebody is looking for.
///
/// A type rather than a `&str` because the matching rule is not a detail of any one call
/// site: it is **plain substring, ASCII case folding, no regex and no word boundaries**,
/// because that is `std.ascii.indexOfIgnoreCase`, which is what libghostty's search does
/// (`observations/libghostty-9f9b8d1d.md` section 10). The renderer highlights what it
/// finds itself and the core says how many there are, so two matchers that disagree would
/// put a count over a screen contradicting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needle {
    text: String,
}

impl Needle {
    pub fn new(text: impl Into<String>) -> Needle {
        Needle { text: text.into() }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Nothing typed yet, which is not the same as a search that found nothing.
    ///
    /// An empty needle matches nowhere rather than everywhere: the alternative is a bar
    /// reporting one hit per row the instant it opens.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// One match, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// How far above the bottom of the pane this row sits.
    ///
    /// The unit the daemon scrolls in, deliberately: a read's lines *are* grid rows, so a
    /// match's place in what was read converts to an offset by counting
    /// (`observations/herdr-0.8.0.md` section 17). The one thing that has to be added is
    /// the blank remainder of the viewport, which herdr trims off the bottom of a read -
    /// see [`found_in`].
    pub rows_from_bottom: u32,

    /// Which bytes of the row matched.
    ///
    /// Bytes rather than cells, because nothing here paints: the renderer is handed the
    /// needle and finds it again itself. Both ends land on character boundaries - a match
    /// can only begin where the needle's first byte does, and a UTF-8 continuation byte is
    /// never the first byte of anything - so this is safe to slice a row with.
    ///
    /// The row itself is not carried. There is no result list to draw, and a common needle
    /// over a thousand rows would mean thousands of copied rows per keystroke.
    pub matched: Range<usize>,
}

/// What searching one pane came back with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    /// Bottom-most match first, and the index climbs the pane from there.
    ///
    /// Ghostty's order, and the find bar borrowed from it draws "next" as an upward
    /// chevron on exactly this basis. In a terminal the interesting match is usually the
    /// recent one, so starting at the bottom is also what somebody pressing ⌘F meant.
    pub hits: Vec<Hit>,

    /// How many rows of text this answer looked at.
    pub rows_searched: u32,

    /// Where the pane was looking, and how much it holds, at the moment it was read.
    ///
    /// Carried with the hits rather than asked for again, because the offsets above are
    /// only meaningful against this viewport: they were worked out from how much the pane
    /// holds, and landing on one has to scroll against the same answer. A second round trip
    /// would let the pane print in between and land somewhere near the match.
    pub viewport: Viewport,

    /// How many grid rows the backend looked at, counting up from the bottom of the pane.
    ///
    /// Not the same as `rows_searched`, and the difference is the whole of the blank-row
    /// correction: the backend looked at this many rows and handed back only the printed
    /// ones. Also what says whether anything was left over, since a backend that reached
    /// every row the pane holds searched the lot.
    pub rows_read: u32,
}

/// How much of a pane a search covered, which is the only thing a person needs told.
///
/// Three answers rather than a flag, because "no results" means something different under
/// each of them and a bar drawing one word cannot work that out from a row count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Every row the pane holds was searched, so "no results" means the text is not there.
    Whole,

    /// The pane holds `rows_held` rows and the backend would not hand them all over, so
    /// there is history this search never reached (`observations/herdr-0.8.0.md` section
    /// 17: herdr clamps a read to a thousand rows and offers no way to page past them).
    Capped { rows_held: u32 },

    /// The pane holds nothing behind the screen you are looking at.
    ///
    /// What a full-screen program leaves - an agent harness, an editor, anything on the
    /// alternate screen - and what an application that erases the scrollback leaves too. The
    /// search was complete and covered one screen, which is the case a person is most likely
    /// to read as "find is broken": they scrolled through that output an hour ago in some
    /// other terminal, and here there is nothing behind it to search.
    ScreenOnly,
}

impl Found {
    /// How much of the pane this answer covered.
    ///
    /// Derived rather than stored, so there is one account of the reach and it is the
    /// numbers the search was actually made of.
    pub fn reach(&self) -> Reach {
        let held = self.viewport.rows_held();
        if held > 0 && self.viewport.deepest == 0 {
            Reach::ScreenOnly
        } else if self.rows_read < held {
            Reach::Capped { rows_held: held }
        } else {
            Reach::Whole
        }
    }
}

/// What a backend's answer holds, once `needle` has been looked for in it.
///
/// `text` is a pane's rows oldest first, newline-separated, exactly as a backend hands
/// them over. `viewport` is where that pane was looking and how much it holds, and
/// `rows_read` is how many grid rows the backend looked at counting up from the bottom -
/// which is the pane's whole height for a read that reached everything, and the backend's
/// own cap for one that did not.
///
/// **The read's bottom row is not the pane's bottom row**, and reconciling the two is what
/// `viewport` and `rows_read` are here for. herdr trims the blank remainder of the viewport
/// off the bottom of what it returns, so a 24-row pane holding three printed rows answers
/// with three (`observations/herdr-0.8.0.md` section 17). Those trimmed rows are added back
/// here, because `rows_from_bottom` is an offset a pane gets scrolled by and one that is 21
/// rows short scrolls to blank space. The count is exact rather than a guess: the backend
/// looked at `rows_read` grid rows and returned the printed ones, so the difference is the
/// blank ones, and it holds whether or not the read hit the cap.
///
/// Within one row the matches come left to right, because a row is read left to right
/// whichever direction the pane is being walked in. Matches do not overlap: after one is
/// found the scan resumes at its end, which is what makes `aa` in `aaaa` two hits and not
/// three.
pub fn found_in(text: &str, needle: &Needle, viewport: Viewport, rows_read: u32) -> Found {
    let rows = rows_of(text);
    let trimmed = rows_read.saturating_sub(counted(rows.len()));
    Found {
        hits: hits_in(&rows, needle, trimmed),
        rows_searched: counted(rows.len()),
        viewport,
        rows_read,
    }
}

/// A row count at the width everything downstream carries it in.
///
/// Infallible in practice - a read is a thousand rows and Muster is the one who asked for
/// that - but saturating rather than wrapping, because a count that silently became small
/// would scroll to a row nowhere near the one that matched.
fn counted(rows: usize) -> u32 {
    u32::try_from(rows).unwrap_or(u32::MAX)
}

/// Every match of `needle` in `rows`, bottom-most first.
///
/// `trimmed` is how many blank rows sit below the last row here, which the backend cut off
/// its answer and this puts back into every offset.
fn hits_in(rows: &[&str], needle: &Needle, trimmed: u32) -> Vec<Hit> {
    if needle.is_empty() {
        return Vec::new();
    }
    let folded = needle.as_str().to_ascii_lowercase();

    // Walked from the bottom up, which is the order the answer is in. Sorting afterwards
    // would do the same thing and would also reorder the matches within a row.
    rows.iter()
        .enumerate()
        .rev()
        .flat_map(|(place, row)| {
            let rows_from_bottom = counted(rows.len() - 1 - place).saturating_add(trimmed);
            matches_in(row, &folded)
                .into_iter()
                .map(move |matched| Hit { rows_from_bottom, matched })
        })
        .collect()
}

/// A pane's text as its rows.
///
/// The trailing newline every read ends with would otherwise become an empty row at the
/// bottom, which would put every real row one further from the bottom than it is - a hit
/// that scrolls to the wrong place, and the kind of off-by-one that looks like the
/// daemon's fault.
///
/// Public because a read that is not a search counts the same rows, and two answers to how
/// many rows a pane just handed back would disagree the moment one of them was fixed.
pub fn rows_of(text: &str) -> Vec<&str> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    if body.is_empty() { Vec::new() } else { body.split('\n').collect() }
}

/// Where a lowercased needle occurs in one row, left to right and without overlapping.
///
/// Folding a byte at a time rather than lowercasing the row, because a row lowercased is
/// an allocation per row per keystroke, and because ASCII folding is the whole rule - a
/// non-ASCII byte compares as itself, which is what libghostty's matcher does and is why
/// `STRASSE` does not find `straße`.
fn matches_in(row: &str, folded_needle: &str) -> Vec<Range<usize>> {
    let row = row.as_bytes();
    let needle = folded_needle.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at + needle.len() <= row.len() {
        let same = row[at..at + needle.len()]
            .iter()
            .zip(needle)
            .all(|(row_byte, needle_byte)| row_byte.to_ascii_lowercase() == *needle_byte);
        if same {
            found.push(at..at + needle.len());
            at += needle.len();
        } else {
            at += 1;
        }
    }
    found
}
