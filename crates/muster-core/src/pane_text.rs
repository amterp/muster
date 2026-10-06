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

/// What of a sent message is looked for on a pane: the tail of it, whitespace removed.
///
/// Whitespace goes because this asks whether text Muster itself just sent came out the other
/// end, against rows a terminal has already reflowed - a message wider than the pane arrives as
/// several rows with a break wherever the wrap fell, and a harness is free to indent it, box it,
/// or fold the run of spaces in it. So both sides have their whitespace removed and what is left
/// is compared as one string. Empty when nothing but keys was sent.
fn looked_for(sent: &str) -> String {
    let characters: Vec<char> = squeezed(sent).chars().collect();
    let from = characters.len().saturating_sub(CONFIRMED_BY);
    characters[from..].iter().collect()
}

/// How many times `wanted` can be seen on a pane, counting runs that do not overlap.
fn times_on(pane: &str, wanted: &str) -> usize {
    squeezed(pane).matches(wanted).count()
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

/// How far up the pane a confirmation reads, before the send and while it waits after. A sent
/// message ends at the bottom of the screen, and a screen is shorter than this at any size
/// somebody works at; the history above is what every re-read would otherwise move, forty times a
/// second. A miss reads the whole pane once before refusing, for a message its own output has
/// already scrolled away.
const CONFIRM_ROWS: u32 = 300;

/// What a pane shows just before something is sent to it, for [`confirm`] to compare against.
///
/// Read only when a caller asked to confirm, and before the send rather than after it, since
/// after is too late to know. A pane that cannot be read counts as blank, which leaves `confirm`
/// asking only whether the text is there at all.
pub fn before_sending(read: impl FnOnce(u32) -> Result<String, String>) -> String {
    read(CONFIRM_ROWS).unwrap_or_default()
}

/// Reads `pane` back until what was just sent to it shows, and says why not when it does not.
///
/// `before` is [`before_sending`]'s read, and `read(rows)` reads the pane's last `rows` rows, or
/// all of it for zero. Shared by every path that sends - through a window, or from the CLI
/// straight to a daemon when no window answers - so a caller asking for the same certainty gets
/// the same answer, in the same words.
///
/// **Against the pane as it was, not merely on it.** Text has arrived once it shows more times
/// than it did before the send, so text that was already there proves nothing: a Codex approval
/// prompt reading `(p)` confirmed a `p` it had ignored, when this asked only whether a `p` was on
/// screen. A send of keys alone has no text to find, and has arrived once the pane changes at
/// all - which proves the pane changed, not that the key changed it, since a spinner changes a
/// pane too. A dialog that ignores what it was sent redraws itself unchanged, which reads as
/// both of these not arriving.
///
/// **It answers arrival and not submission.** A pane draws the text whether it has been
/// submitted or is sitting in an input box waiting for a Return, and nothing in a pane's
/// rendered rows separates those. A harness that folds a long paste into a placeholder draws
/// neither, which reads here as not arrived - the honest answer, since a caller that cannot
/// see its message on the pane has not confirmed anything.
///
/// Reading until [`CONFIRM_WITHIN`] runs out rather than once, because a send is accepted before
/// the pane can have drawn it. A pane that has drawn it answers on the first read, so the cost
/// falls on the miss.
pub fn confirm(
    pane: &str,
    sent: &str,
    before: &str,
    mut read: impl FnMut(u32) -> Result<String, String>,
) -> Result<(), String> {
    let wanted = looked_for(sent);
    let shown_before = times_on(before, &wanted);
    let arrived = |after: &str| {
        if wanted.is_empty() { after != before } else { times_on(after, &wanted) > shown_before }
    };
    let deadline = std::time::Instant::now() + CONFIRM_WITHIN;
    // Whatever the last read said, so a pane that could not be read at all is reported as that
    // rather than as one that drew nothing - two different things to be told.
    let unreadable = loop {
        let outcome = match read(CONFIRM_ROWS) {
            Ok(text) if arrived(&text) => return Ok(()),
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
    // the history only on the path that has already waited out the second. Only for text that
    // was nowhere in the rows before the send: older copies further up would be counted too.
    if unreadable.is_none()
        && !wanted.is_empty()
        && shown_before == 0
        && read(0).is_ok_and(|text| times_on(&text, &wanted) > 0)
    {
        return Ok(());
    }
    Err(match unreadable {
        None if wanted.is_empty() => format!(
            "the keys were sent to pane {pane} and nothing on it changed, so whatever is running \
             there did not act on them. A program draws nothing for a key it ignores, and \
             nothing at all while it is not reading its input. `muster pane read --pane {pane}` \
             shows what it does draw."
        ),
        None => format!(
            "the text was sent to pane {pane} and does not show on it any more than it did \
             before, so whatever is running there did not take it. Three things do this. A \
             dialog or menu redraws itself unchanged when it is pasted at, since it reads keys: \
             `--key` presses them. A terminal in canonical mode - anything reading stdin without \
             a line editor - discards a line over 1024 bytes whole rather than cutting it, and \
             says nothing (`muster docs limits`). And a harness that folds a long paste into a \
             placeholder draws neither the text nor an error, which reads here the same way. \
             `muster pane read --pane {pane}` shows what it does draw."
        ),
        Some(refusal) => format!(
            "what was sent to pane {pane} could not be confirmed, because reading the pane back \
             failed: {refusal}. Whatever was sent may well have arrived."
        ),
    })
}

/// One string with every whitespace character taken out of it.
fn squeezed(text: &str) -> String {
    text.chars().filter(|character| !character.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::{PaneText, before_sending, confirm};

    /// A send the pane draws late is confirmed once it does, and one it never draws is refused
    /// only after the whole pane has been read as well.
    #[test]
    fn a_confirmation_waits_for_the_pane_and_reads_it_whole_before_refusing() {
        let mut reads = 0;
        assert_eq!(
            confirm("p1", "hello", "$ ", |_| {
                reads += 1;
                Ok(if reads < 3 { "$ ".to_string() } else { "$ hello\n".to_string() })
            }),
            Ok(())
        );
        let mut asked = Vec::new();
        let missed = confirm("p1", "hello", "$ ", |rows| {
            asked.push(rows);
            Ok("$ ".to_string())
        });
        assert!(missed.is_err_and(|refusal| refusal.contains("does not show on it")));
        assert_eq!(asked.last(), Some(&0), "the last read is of the whole pane");
        let unreadable = confirm("p1", "hello", "$ ", |_| Err("gone".to_string()));
        assert!(unreadable.is_err_and(|refusal| refusal.contains("failed: gone")));
    }

    /// The case that made this compare: an approval prompt that ignored a pasted `p` was
    /// confirmed, because its own `(p)` was already on screen.
    #[test]
    fn text_that_was_already_on_the_pane_confirms_only_once_it_shows_again() {
        let prompt = "2. Yes, and don't ask again (p)\n3. No (esc)\n";
        let ignored = confirm("p1", "p", prompt, |_| Ok(prompt.to_string()));
        assert!(ignored.is_err_and(|refusal| refusal.contains("--key")));
        let typed = format!("{prompt}> p\n");
        assert_eq!(confirm("p1", "p", prompt, |_| Ok(typed.clone())), Ok(()));
    }

    /// Text that was on the pane before is not looked for in the whole history either, where
    /// older copies would confirm a send that never arrived.
    #[test]
    fn a_miss_reads_the_whole_pane_only_for_text_that_was_not_there_before() {
        let mut asked = Vec::new();
        let missed = confirm("p1", "ls", "$ ls\n", |rows| {
            asked.push(rows);
            Ok(if rows == 0 { "$ ls\n$ ls\n".to_string() } else { "$ ls\n".to_string() })
        });
        assert!(missed.is_err());
        assert!(!asked.contains(&0), "the whole pane holds the old copy, so it is not read");
        let scrolled = confirm("p1", "make", "$ ", |rows| {
            Ok(if rows == 0 { "$ make\nlots\n".to_string() } else { "lots\n".to_string() })
        });
        assert_eq!(scrolled, Ok(()), "output that scrolled the text away does not hide it");
    }

    /// Keys have no text to find, so what shows they arrived is the pane changing at all.
    #[test]
    fn keys_alone_are_confirmed_by_the_pane_changing() {
        let dialog = "\u{276f} No, exit\n  Yes, I trust this folder\n";
        let moved = "  No, exit\n\u{276f} Yes, I trust this folder\n";
        assert_eq!(confirm("p1", "", dialog, |_| Ok(moved.to_string())), Ok(()));
        let unchanged = confirm("p1", "", dialog, |_| Ok(dialog.to_string()));
        assert!(unchanged.is_err_and(|refusal| refusal.contains("nothing on it changed")));
    }

    #[test]
    fn a_pane_unreadable_before_the_send_leaves_only_whether_the_text_is_there() {
        assert_eq!(before_sending(|_| Err("gone".to_string())), "");
        assert_eq!(confirm("p1", "hello", "", |_| Ok("$ hello\n".to_string())), Ok(()));
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
