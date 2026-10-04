//! Reading a pane's newest rows through the daemon's pages.
//!
//! A read answers one page of at most 4 MiB, so a pane holding more takes several, and a daemon
//! that predates reading from the end answers from the first row instead. Every client that reads
//! a pane - the app, and the CLI when no window answers - pages the same way, so this is here
//! rather than in either of them.

use crate::PaneText;

/// A pane's newest rows as one text, and whether the pane holds rows older than it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Newest {
    pub text: String,
    pub truncated: bool,
}

/// The last `rows` rows of a pane, or as much of its history as one answer holds for zero.
///
/// `read_page(first_row, last)` asks the daemon for one page: from `first_row`, or the last
/// `last` rows when that is not zero.
pub fn newest<E>(
    rows: u32,
    mut read_page: impl FnMut(u64, u32) -> Result<PaneText, E>,
) -> Result<Newest, E> {
    // Only the rows wanted, when some are: a pane's whole history is what a starved reader
    // would otherwise have to drain before it could answer, and twenty rows fit a socket's
    // buffer where 12000 do not.
    let first = read_page(0, rows)?;
    if answers_the_tail(&first, rows) {
        return Ok(Newest { truncated: first.first_row > 0, text: first.text });
    }
    if reaches_the_end(&first) {
        return Ok(Newest { text: first.text, truncated: false });
    }
    // A page stops at the daemon's 4 MiB, and what a read is for is the newest rows, so the
    // page wanted is the one ending at the last row. Rows differ in length, so start as far
    // from the end as the first page reached from the start, and move on by however far a
    // page still falls short. A few tries settle it; the cap only bounds a pane printing
    // faster than it can be read.
    let mut newest = first;
    for _ in 0..8 {
        let first_row = newest.total_rows.saturating_sub(u64::from(newest.rows));
        newest = read_page(first_row, 0)?;
        if reaches_the_end(&newest) || newest.rows == 0 {
            break;
        }
    }
    Ok(Newest { text: newest.text, truncated: true })
}

/// What a pane's agent printed in its last turn, from the one page `read_turn` asks the daemon
/// for. None when the daemon answered without placing a turn: it predates the request, and read
/// something else.
pub fn turn<E>(read_turn: impl FnOnce() -> Result<PaneText, E>) -> Result<Option<Newest>, E> {
    let page = read_turn()?;
    Ok(page.turn.map(|start| Newest { truncated: page.first_row > start, text: page.text }))
}

/// Whether a read that asked for the last `rows` rows got them.
///
/// A daemon that predates reading from the end reads from the first row instead. It gives that
/// away by sending more rows than were asked for, or, when its 4 MiB cut the page short of that,
/// by starting at row 0 and ending far from the last row. A tail ends at the last row with
/// anything on it, so at most a screen of blank rows short of the end.
fn answers_the_tail(page: &PaneText, rows: u32) -> bool {
    /// Taller than any screen, and far shorter than the rows a 4 MiB cut leaves out.
    const A_SCREEN: u64 = 1000;
    let ends = page.first_row + u64::from(page.rows);
    rows > 0 && page.rows <= rows && (page.first_row > 0 || ends + A_SCREEN >= page.total_rows)
}

fn reaches_the_end(page: &PaneText) -> bool {
    page.first_row + u64::from(page.rows) >= page.total_rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(first_row: u64, rows: u32, total_rows: u64) -> PaneText {
        PaneText { first_row, total_rows, rows, ..PaneText::default() }
    }

    #[test]
    fn a_tail_is_told_from_an_older_daemons_first_page() {
        // A daemon that reads from the end: the last rows with text, a screen of blank rows
        // below them left out, or everything when the pane holds fewer.
        assert!(answers_the_tail(&page(9_980, 20, 10_024), 20));
        assert!(answers_the_tail(&page(0, 12, 40), 20));
        // An older daemon answers from row 0, cut at its 4 MiB, which can still be fewer rows
        // than a large count asked for. Those are the oldest rows, not the newest.
        assert!(!answers_the_tail(&page(0, 42_000, 100_000), 100_000));
        // And more rows than were asked for is an older daemon's whole answer.
        assert!(!answers_the_tail(&page(0, 300, 300), 20));
    }

    #[test]
    fn a_whole_history_past_one_page_is_read_from_the_page_ending_at_the_last_row() {
        let mut asked = Vec::new();
        let read = newest(0, |first_row, last| {
            asked.push((first_row, last));
            // 100 rows in all, and a page holds 40 of them.
            let from = first_row.min(100);
            let rows = (100 - from).min(40);
            Ok::<_, ()>(PaneText {
                first_row: from,
                rows: u32::try_from(rows).unwrap(),
                total_rows: 100,
                text: format!("rows {from}.."),
                turn: None,
            })
        })
        .unwrap();
        assert_eq!(read, Newest { text: "rows 60..".to_string(), truncated: true });
        assert_eq!(asked, [(0, 0), (60, 0)]);
    }

    #[test]
    fn a_turn_is_cut_short_only_when_its_page_starts_after_it_and_unplaced_when_not_answered() {
        let at = |first_row: u64, started: Option<u64>| {
            let page = PaneText { first_row, turn: started, ..PaneText::default() };
            turn(|| Ok::<_, ()>(page)).unwrap()
        };
        assert_eq!(at(40, Some(40)).map(|read| read.truncated), Some(false));
        assert_eq!(at(55, Some(40)).map(|read| read.truncated), Some(true), "4 MiB cut its top");
        assert_eq!(at(0, None), None, "a daemon that predates the field read something else");
    }
}
