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
//! **The reach is honest rather than complete.** herdr answers at most a thousand rows
//! and offers no way to page past them (`observations/herdr-0.8.0.md` section 17), so a
//! long pane is searched in part - and `Found::truncated` is how whoever draws the answer
//! knows to say so. A confident "no results" over four fifths of a pane is worse than no
//! find at all, which is the whole reason that flag is carried up rather than dropped
//! here.

use std::ops::Range;

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
    /// match's place in what was read is already the offset to scroll to and nothing
    /// converts it (`observations/herdr-0.8.0.md` section 17).
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

    /// How many rows this answer looked at.
    pub rows_searched: u32,

    /// Whether the pane holds history this search never reached.
    ///
    /// The backend's own answer rather than a guess from the row count: herdr sets it
    /// whenever there is more than it returned, which is the question a person wants
    /// answered and not the same as "the cap was hit".
    pub truncated: bool,
}

/// What a backend's answer holds, once `needle` has been looked for in it.
///
/// `text` is a pane's rows oldest first, newline-separated, exactly as a backend hands
/// them over - so the last row is the bottom of the pane and is `rows_from_bottom` zero.
/// A single trailing newline is the end of the last row rather than an empty row after it.
///
/// `truncated` rides along rather than being worked out from the row count, because only
/// whoever asked knows whether there was more: a thousand rows back may be all a pane has
/// or a fifth of it, and those are different answers to show a person.
///
/// Within one row the matches come left to right, because a row is read left to right
/// whichever direction the pane is being walked in. Matches do not overlap: after one is
/// found the scan resumes at its end, which is what makes `aa` in `aaaa` two hits and not
/// three.
pub fn found_in(text: &str, needle: &Needle, truncated: bool) -> Found {
    let rows = rows_of(text);
    Found { hits: hits_in(&rows, needle), rows_searched: counted(rows.len()), truncated }
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
fn hits_in(rows: &[&str], needle: &Needle) -> Vec<Hit> {
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
            let rows_from_bottom = counted(rows.len() - 1 - place);
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
fn rows_of(text: &str) -> Vec<&str> {
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
