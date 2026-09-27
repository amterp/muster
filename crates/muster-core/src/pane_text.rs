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
    /// So Muster asks for as far back as the backend will go and counts here, where a row is
    /// a row of text rather than a cell in a grid, split by [`rows_of`].
    ///
    /// The cost is stated rather than hidden: every read is the pane's whole history on the
    /// wire, up to the 4 MiB a daemon puts in one page. That is a bounded, human-frequency
    /// request - a person or an agent asking what a pane has printed - rather than anything on
    /// the render path.
    #[must_use]
    pub fn tail(self, rows: u32) -> PaneText {
        let held = rows_of(&self.text);
        let wanted = rows as usize;
        if rows == 0 || held.len() <= wanted {
            return self;
        }
        PaneText {
            text: held[held.len() - wanted..].join("\n") + "\n",
            // There is history this answer did not reach, because Muster dropped it.
            truncated: true,
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

/// One string with every whitespace character taken out of it.
fn squeezed(text: &str) -> String {
    text.chars().filter(|character| !character.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::PaneText;

    fn read(text: &str, truncated: bool) -> PaneText {
        PaneText { text: text.to_string(), truncated }
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

    /// A backend that truncated says so whatever the count does. The two are different
    /// claims about the same sentence - one is what the backend could not reach, the other
    /// what Muster chose not to hand over - and either one makes it true.
    #[test]
    fn a_backends_own_truncation_survives_a_count_that_cut_nothing() {
        assert!(read("one\ntwo\n", true).tail(50).truncated);
        assert!(read("one\ntwo\n", true).tail(0).truncated);
    }
}
