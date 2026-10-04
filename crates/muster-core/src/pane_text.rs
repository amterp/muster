//! What a pane has printed, as it is read back, and the rules for reading it.

/// What a pane has printed, as far back as the backend would go.
///
/// Asked for rather than followed: a pane's output never enters the core (`architecture.md`, control plane and data plane), so this is the
/// one place its text is read at all - and it is read at the moment somebody asks rather than
/// held, because holding it would be a copy of every pane's history going stale between reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneText {
    /// Newest row last, the way the pane draws it.
    pub text: String,

    /// Whether the pane holds history this read never reached.
    ///
    /// The backend's own answer rather than a guess from the row count: only the backend knows
    /// whether the rows it handed back are all it holds. [`PaneText::tail`] can also
    /// set it, because a caller that asked for forty rows out of a hundred did not reach the
    /// other sixty either.
    pub truncated: bool,

    /// Whether these are the rows the pane's agent printed in its last turn ([`Scope::Turn`]).
    pub turn: bool,
}

/// Which of a pane's rows a read is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The newest this many rows, or as much as the backend holds for zero.
    Newest(u32),
    /// What the pane's agent printed since it last went to work.
    Turn,
}

impl PaneText {
    /// The last `rows` rows of what a backend handed back, or all of it for zero.
    ///
    /// A count rather than a ceiling, and that distinction is the whole of why this exists.
    /// A backend counts rows in the grid it draws, and the bottom of an idle pane is the
    /// blank remainder of its screen - so asking a daemon for the last forty rows of a
    /// pane sitting at a prompt buys forty blank ones, which trim away to nothing. Blank is
    /// byte-identical to a pane that has printed nothing, and it cost this card's author a
    /// near-miss on closing a shell with twenty-four lines on it.
    ///
    /// So Muster counts here, where a row is a row of text rather than a cell in a grid, split
    /// by [`rows_of`]. A daemon is asked for only the rows wanted, and ends them at the last row
    /// with text as this does; one that predates that sends the whole history, which this cuts.
    /// The whole history was once every read, and beside a busy build it was the 240 KB a
    /// starved window had to drain before a twenty-row read could answer.
    ///
    /// And it counts from the last row with anything on it, for the same reason: the rows
    /// beneath a prompt are the blank rest of the screen, which a backend hands back as rows.
    /// Leaving them out is not truncation, since nothing was printed there.
    #[must_use]
    pub fn tail(self, rows: u32) -> PaneText {
        let held = rows_of(&self.text);
        let written =
            held.iter().rposition(|row| !row.trim().is_empty()).map_or(0, |last| last + 1);
        let from = if rows == 0 { 0 } else { written.saturating_sub(rows as usize) };
        if from == 0 && written == held.len() {
            return self;
        }
        PaneText {
            text: if written == from {
                String::new()
            } else {
                held[from..written].join("\n") + "\n"
            },
            // There is history this answer did not reach, because Muster dropped it.
            truncated: self.truncated || from > 0,
            turn: self.turn,
        }
    }
}

/// A pane's text as its rows.
///
/// The trailing newline every read ends with would otherwise become an empty row at the
/// bottom, which would put every real row one further from the bottom than it is, and the
/// kind of off-by-one that looks like the daemon's fault.
///
/// Public because a caller reporting how many rows a pane handed back counts the same rows,
/// and two answers would disagree the moment one of them was fixed.
pub fn rows_of(text: &str) -> Vec<&str> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    if body.is_empty() { Vec::new() } else { body.split('\n').collect() }
}

/// How much of a sent message is looked for when confirming it arrived.
///
/// A tail rather than the whole thing, because a message can be longer than the pane is tall
/// and its beginning may have scrolled off - and because the tail is exactly what a discarded
/// line loses. Long enough to be distinctive against a screen of an agent's own output; short
/// enough to fit inside a pane at any width somebody works at.
const CONFIRMED_BY: usize = 120;

/// Whether a message that was sent to a pane can be seen on it.
///
/// Not a search, and the difference is whitespace. It asks whether text Muster itself just sent
/// came out the other end, against rows a terminal has already reflowed - a message wider than
/// the pane arrives as several rows with a break wherever the wrap fell, and a harness is free
/// to indent it, box it, or fold the run of spaces in it. So both sides have their whitespace
/// removed and what is left is compared as one string.
///
/// **It answers arrival and not submission.** A pane draws the text whether it has been
/// submitted or is sitting in an input box waiting for a Return, and nothing in a pane's
/// rendered rows separates those. A harness that folds a long paste into a placeholder draws
/// neither, which reads here as not arrived - the honest answer, since a caller that cannot
/// see its message on the pane has not confirmed anything.
///
/// Empty text arrives trivially: `pane send --enter ''` is how a caller presses Return on its
/// own, and there is nothing to look for.
pub fn arrived_in(pane: &str, sent: &str) -> bool {
    let wanted = squeezed(sent);
    if wanted.is_empty() {
        return true;
    }
    let characters: Vec<char> = wanted.chars().collect();
    let from = characters.len().saturating_sub(CONFIRMED_BY);
    let tail: String = characters[from..].iter().collect();
    squeezed(pane).contains(&tail)
}

/// How long a pane is given to draw what it was handed before `--confirm` calls it missing.
///
/// A send is accepted once it is queued for the daemon, which is before the program has read
/// it, echoed it, and had that land in the daemon's copy of the screen. Reading once at that
/// moment refuses whatever has not finished the trip - the delivered message reported as
/// refused, which is the one answer confirming exists to make impossible.
///
/// The floor is the slowest an honest pane was measured taking: 54ms, against a median of 0,
/// with sixteen spinning cores on a ten-core machine. The ceiling is what a caller driving
/// several agents will pay on a send that genuinely did not land, because only a miss waits out
/// the whole budget - a pane that has already drawn the text answers on the first read, before
/// any sleep. A second buys about eighteen times the worst honest case for a price a caller
/// pays only when the news is bad.
pub const CONFIRM_WITHIN: std::time::Duration = std::time::Duration::from_secs(1);

/// How often the pane is re-read while waiting, matching the seam's other bounded waits.
const CONFIRM_POLL: std::time::Duration = std::time::Duration::from_millis(25);

/// How far up the pane a confirmation reads while it waits. A sent message ends at the bottom of
/// the screen, and a screen is shorter than this at any size somebody works at; the history above
/// is what every re-read would otherwise move, forty times a second. A miss reads the whole pane
/// once before refusing, for a message its own output has already scrolled away.
const CONFIRM_ROWS: u32 = 300;

/// Reads `pane` back until what was just sent to it appears, and says why not when it does not.
///
/// `read(rows)` reads the pane's last `rows` rows, or all of it for zero. Shared by every path
/// that sends - through a window, or from the CLI straight to a daemon when no window answers -
/// so a caller asking for the same certainty gets the same answer, in the same words.
///
/// Reading until [`CONFIRM_WITHIN`] runs out rather than once, because a send is accepted before
/// the pane can have drawn it. A pane that has drawn it answers on the first read, so the cost
/// falls on the miss.
///
/// The refusal says what it looked for, since the commonest cause is a pane whose harness draws
/// the text somewhere this cannot read - which is a fact about that harness rather than about
/// the send.
pub fn confirm(
    pane: &str,
    sent: &str,
    mut read: impl FnMut(u32) -> Result<String, String>,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + CONFIRM_WITHIN;
    // Whatever the last read said, so a pane that could not be read at all is reported as that
    // rather than as one that drew nothing - two different things to be told.
    let unreadable = loop {
        let outcome = match read(CONFIRM_ROWS) {
            Ok(text) if arrived_in(&text, sent) => return Ok(()),
            Ok(_) => None,
            Err(refusal) => Some(refusal),
        };
        if std::time::Instant::now() >= deadline {
            break outcome;
        }
        std::thread::sleep(CONFIRM_POLL);
    };
    // A command whose output scrolled the message past the last rows ran, and refusing it
    // invites running it twice. So the whole pane is read once before saying so, which costs
    // the history only on the path that has already waited out the second.
    if unreadable.is_none() && read(0).is_ok_and(|text| arrived_in(&text, sent)) {
        return Ok(());
    }
    Err(match unreadable {
        None => format!(
            "the text was sent to pane {pane} and is not on it, so whatever is running there \
             did not receive it. Two things do this. A terminal in canonical mode - anything \
             reading stdin without a line editor - discards a line over 1024 bytes whole rather \
             than cutting it, and says nothing (`muster docs limits`). And a harness that folds \
             a long paste into a placeholder draws neither the text nor an error, which reads \
             here the same way. `muster pane read --pane {pane}` shows what it does draw."
        ),
        Some(refusal) => format!(
            "the text was sent to pane {pane} and reading it back to confirm failed: {refusal}. \
             Whatever was sent may well have arrived."
        ),
    })
}

/// One string with every whitespace character taken out of it.
fn squeezed(text: &str) -> String {
    text.chars().filter(|character| !character.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::{PaneText, confirm};

    /// A send the pane draws late is confirmed once it does, and one it never draws is refused
    /// only after the whole pane has been read as well.
    #[test]
    fn a_confirmation_waits_for_the_pane_and_reads_it_whole_before_refusing() {
        let mut reads = 0;
        assert_eq!(
            confirm("p1", "hello", |_| {
                reads += 1;
                Ok(if reads < 3 { "$ ".to_string() } else { "$ hello\n".to_string() })
            }),
            Ok(())
        );
        let mut asked = Vec::new();
        let missed = confirm("p1", "hello", |rows| {
            asked.push(rows);
            Ok("$ ".to_string())
        });
        assert!(missed.is_err_and(|refusal| refusal.contains("is not on it")));
        assert_eq!(asked.last(), Some(&0), "the last read is of the whole pane");
        let unreadable = confirm("p1", "hello", |_| Err("gone".to_string()));
        assert!(unreadable.is_err_and(|refusal| refusal.contains("failed: gone")));
    }

    fn read(text: &str, truncated: bool) -> PaneText {
        PaneText { text: text.to_string(), truncated, turn: false }
    }

    /// The bug this exists for, in miniature: a caller asks for fewer rows than the pane holds
    /// and gets the newest ones, which is what "the last forty" has always meant to whoever
    /// typed it.
    #[test]
    fn a_count_takes_the_newest_rows() {
        let cut = read("one\ntwo\nthree\nfour\n", false).tail(2);
        assert_eq!(cut.text, "three\nfour\n");
        assert!(
            cut.truncated,
            "two rows were left above, which is history this read did not reach"
        );
    }

    /// Asking for more than there is is not an error and does not pad. It is also the case
    /// that must not set `truncated`: a caller told there is more when there is not stops
    /// believing the flag on the reads where it matters.
    #[test]
    fn asking_for_more_than_there_is_answers_with_what_there_is() {
        let whole = read("one\ntwo\n", false).tail(50);
        assert_eq!(whole.text, "one\ntwo\n");
        assert!(!whole.truncated);
    }

    /// Zero is "as far back as you have", which is what `pane read` with no `--rows` means.
    #[test]
    fn zero_is_everything() {
        assert_eq!(read("one\ntwo\n", false).tail(0).text, "one\ntwo\n");
    }

    /// The rows beneath a prompt are the blank rest of the screen rather than rows anything
    /// printed, so the count starts from the last row with anything on it. Counted as rows, a
    /// pane that had just cleared its screen answered three blank ones, which reads exactly
    /// like a pane that printed nothing.
    #[test]
    fn a_count_starts_at_the_last_row_with_anything_on_it() {
        let cut = read("one\ntwo\n❯ \n\n  \n\n", false).tail(2);
        assert_eq!(cut.text, "two\n❯ \n");
        assert!(cut.truncated, "one row was left above");
        let whole = read("❯\n\n\n", false).tail(0);
        assert_eq!(whole.text, "❯\n", "nor does everything end in the blank rest of the screen");
        assert!(!whole.truncated, "blank rows left out are not history left out");
        assert_eq!(read("a\n\nb\n\n", false).tail(3).text, "a\n\nb\n", "a blank row between two");
        assert_eq!(read("\n\n", false).tail(5).text, "", "a blank screen is nothing");
    }

    /// A backend that truncated says so whatever the count does. The two are different
    /// claims about the same sentence - one is what the backend could not reach, the other
    /// what Muster chose not to hand over - and either one makes it true.
    #[test]
    fn a_backends_own_truncation_survives_a_count_that_cut_nothing() {
        assert!(read("one\ntwo\n", true).tail(50).truncated);
        assert!(read("one\ntwo\n", true).tail(0).truncated);
    }
}
