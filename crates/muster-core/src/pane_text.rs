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

/// How much of the start of a sent message is looked for, to tell a message cut short from one
/// that was taken and drawn over.
///
/// Shorter than any program truncates to in practice - a terminal in canonical mode echoes the
/// first 1024 bytes of a line it then drops - and long enough that a match is this send's text
/// rather than the program's own. Text this short or shorter is its own start.
const STARTED_BY: usize = 16;

/// What of the start of a sent message is looked for, as [`looked_for`] takes its tail.
fn start_of(sent: &str) -> String {
    squeezed(sent).chars().take(STARTED_BY).collect()
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

/// Whether a send is checked against what its pane's agent's prompt already holds: one that
/// types text, unless asked to add to it. Keys and Return alone are how a dialog is answered and
/// a prompt submitted, and so is a lone space with Return, which submits a Claude Code prompt
/// where Return alone does not, so text that is only whitespace is not checked either.
#[must_use]
pub fn checks_prompt(text: &str, append: bool) -> bool {
    !append && !text.trim().is_empty()
}

/// Refuses text sent into a prompt that already holds `held`, which would be submitted with
/// it as one prompt by the next Return: a draft somebody was writing, or a wake left unsent.
pub fn into_held_prompt(pane: &str, held: Option<&str>) -> Result<(), String> {
    const SHOWN: usize = 120;
    let Some(held) = held else { return Ok(()) };
    let shown: String = held.chars().take(SHOWN).collect();
    let cut = if held.chars().count() > SHOWN { "..." } else { "" };
    Err(format!(
        "nothing was sent: the prompt in pane {pane} already holds \"{shown}{cut}\", and what \
         you sent would be added to it and submitted with it. Clear the prompt first, or send \
         with --append to add to what it holds."
    ))
}

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
/// **Short text is common text.** A `y` or a `2` already shows many times in the rows read, and
/// an agent printing after it scrolls old copies out as fast as new ones arrive, so its count can
/// fall although it arrived - and a refusal invites the caller to send it twice. So text that was
/// already there also counts as arrived once the pane changes and still shows it, which is as
/// much as keys alone can prove. A pane that does not change still refuses it.
///
/// **It answers arrival and not submission.** A pane draws the text whether it has been
/// submitted or is sitting in an input box waiting for a Return, and nothing in a pane's
/// rendered rows separates those.
///
/// **A pane that changed without showing the text is not a refusal.** A program that takes a
/// line and redraws - Codex quitting to the shell on `/quit`, a slash command leaving its prompt
/// empty, a prompt submitted with its Return - shows none of what it took, and calling that "did
/// not take it" sent callers to send it again, into whatever was running by then. A harness that
/// folds a long paste into a placeholder, and a busy agent drawing over its input, look the same.
/// So that is [`Unconfirmed::Unknown`], which the CLI exits 4 for, as it does for any request
/// that may have happened. Refused is kept for what the pane does show: nothing changed at all,
/// or the start of the text arrived and its end did not, which is a line cut short. Text that
/// never shows is still never confirmed, so a busy pane that loses what it was sent is no easier
/// to confirm than it was.
///
/// Reading until [`CONFIRM_WITHIN`] runs out rather than once, because a send is accepted before
/// the pane can have drawn it. A pane that has drawn it answers on the first read, so the cost
/// falls on the miss.
pub fn confirm(
    pane: &str,
    sent: &str,
    before: &str,
    mut read: impl FnMut(u32) -> Result<String, String>,
) -> Result<(), Unconfirmed> {
    let wanted = looked_for(sent);
    let shown_before = times_on(before, &wanted);
    let arrived = |after: &str| {
        if wanted.is_empty() {
            return after != before;
        }
        let shown = times_on(after, &wanted);
        shown > shown_before || (shown_before > 0 && shown > 0 && after != before)
    };
    let start = start_of(sent);
    let started_before = times_on(before, &start);
    let mut changed = false;
    // Only the last read decides whether the text was cut short: a harness can draw the start
    // of a paste on its way in and then clear the whole of it on the Return.
    let mut started = false;
    let deadline = std::time::Instant::now() + CONFIRM_WITHIN;
    // Whatever the last read said, so a pane that could not be read at all is reported as that
    // rather than as one that drew nothing - two different things to be told.
    let unreadable = loop {
        let outcome = match read(CONFIRM_ROWS) {
            Ok(text) if arrived(&text) => return Ok(()),
            Ok(text) => {
                changed |= text != before;
                started = times_on(&text, &start) > started_before;
                None
            }
            Err(refusal) => Some(refusal),
        };
        if std::time::Instant::now() >= deadline {
            break outcome;
        }
        std::thread::sleep(CONFIRM_POLL);
    };
    if let Some(refusal) = unreadable {
        return Err(Unconfirmed::Unknown(format!(
            "what was sent to pane {pane} could not be confirmed, because reading the pane back \
             failed: {refusal}. Whatever was sent may well have arrived."
        )));
    }
    // A command whose output scrolled the message past the last rows ran, and refusing it
    // invites running it twice. So the whole pane is read once before saying so, which costs
    // the history only on the path that has already waited out the second. Only for text that
    // was nowhere in the rows before the send: older copies further up would be counted too.
    if !wanted.is_empty()
        && shown_before == 0
        && read(0).is_ok_and(|text| times_on(&text, &wanted) > 0)
    {
        return Ok(());
    }
    Err(if wanted.is_empty() {
        Unconfirmed::Refused(format!(
            "the keys were sent to pane {pane} and nothing on it changed, so whatever is running \
             there did not act on them. A program draws nothing for a key it ignores, and \
             nothing at all while it is not reading its input. `muster pane read --pane {pane}` \
             shows what it does draw."
        ))
    } else if started {
        Unconfirmed::Refused(format!(
            "the text was sent to pane {pane} and only its start shows on it, so whatever is \
             running there did not take the rest. A terminal in canonical mode - anything \
             reading stdin without a line editor - does this to a line over 1024 bytes: it \
             echoes the start, delivers none of it, says nothing, and takes nothing sent after \
             it until `--key ctrl+c` interrupts what is reading (`muster docs limits`). \
             `muster pane read --pane {pane}` shows what it does draw."
        ))
    } else if !changed {
        Unconfirmed::Refused(format!(
            "the text was sent to pane {pane} and nothing on it changed, so whatever is running \
             there did not take it. Two things do this. A dialog or menu redraws itself \
             unchanged when it is pasted at, since it reads keys: `--key` presses them. And a \
             terminal in canonical mode - anything reading stdin without a line editor - that \
             was sent a line over 1024 bytes takes nothing more until `--key ctrl+c` interrupts \
             what is reading (`muster docs limits`). `muster pane read --pane {pane}` shows what \
             it does draw."
        ))
    } else {
        Unconfirmed::Unknown(format!(
            "the text was sent to pane {pane}, and the pane changed without showing it, so \
             whether it was taken cannot be told from the screen. A program that takes a line \
             and redraws looks like this - a slash command, a quit, a prompt submitted with its \
             Return - and so do a harness that folds a long paste into a placeholder and a busy \
             agent drawing over its input. Do not send it again until `muster pane read --pane \
             {pane}` shows which: sent twice, it may be taken twice."
        ))
    })
}

/// Why [`confirm`] could not confirm a send, by what a caller should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unconfirmed {
    /// The pane shows the send was not taken, so sending it again, or as keys, is safe.
    Refused(String),
    /// The send may have been taken: the pane changed without showing it, or could not be read
    /// back. Sending it again may deliver it twice.
    Unknown(String),
}

impl Unconfirmed {
    pub fn reason(&self) -> &str {
        match self {
            Unconfirmed::Refused(reason) | Unconfirmed::Unknown(reason) => reason,
        }
    }
}

/// One string with every whitespace character taken out of it.
fn squeezed(text: &str) -> String {
    text.chars().filter(|character| !character.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::{PaneText, Unconfirmed, before_sending, checks_prompt, confirm, into_held_prompt};

    /// Text is checked against a prompt holding something; keys, Return and a lone space with
    /// Return are not, and a send told to append is not.
    #[test]
    fn only_text_is_held_back_by_a_prompt_that_holds_something() {
        assert!(checks_prompt("/clear", false));
        assert!(!checks_prompt("/clear", true));
        assert!(!checks_prompt("", false));
        assert!(!checks_prompt(" ", false));
        assert_eq!(into_held_prompt("p1", None), Ok(()));
        let refused = into_held_prompt("p1", Some("[muster] review: 1 new (#4)")).unwrap_err();
        assert!(refused.contains("already holds \"[muster] review: 1 new (#4)\""), "{refused}");
        assert!(refused.contains("--append"), "{refused}");
        let long = "x".repeat(500);
        let refused = into_held_prompt("p1", Some(&long)).unwrap_err();
        assert!(refused.contains(&format!("{}...\"", "x".repeat(120))), "{refused}");
    }

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
        assert!(refused(missed, "nothing on it changed"));
        assert_eq!(asked.last(), Some(&0), "the last read is of the whole pane");
    }

    fn refused(outcome: Result<(), Unconfirmed>, saying: &str) -> bool {
        matches!(outcome, Err(Unconfirmed::Refused(reason)) if reason.contains(saying))
    }

    fn unknown(outcome: Result<(), Unconfirmed>, saying: &str) -> bool {
        matches!(outcome, Err(Unconfirmed::Unknown(reason)) if reason.contains(saying))
    }

    /// Codex took `/quit` and exited to the shell, and took `/compact` and redrew its prompt
    /// empty. Neither shows what it took, and neither may be called a refusal: the caller's next
    /// step would be to send it again, into the shell.
    #[test]
    fn a_line_a_program_took_and_redrew_away_is_unknown_rather_than_refused() {
        let codex = "\u{203a} Ask Codex to do anything\n  100% context left\n";
        let quit = confirm("p1", "/quit", codex, |_| Ok("$ \n".to_string()));
        assert!(unknown(quit, "cannot be told from the screen"));
        let compacted = "\u{2022} Compacting context\n\u{203a} \n  0% context used\n";
        let compact = confirm("p1", "/compact", codex, |_| Ok(compacted.to_string()));
        assert!(unknown(compact, "Do not send it again"));
    }

    /// The opposite failure stays shut: a busy pane changes on every read, and its change is no
    /// evidence that text it never showed arrived.
    #[test]
    fn a_busy_pane_that_never_shows_the_text_is_never_confirmed() {
        let mut frame = 0;
        let spinning = confirm("p1", "fix the build", "\u{2733} Working (1s)\n", |_| {
            frame += 1;
            Ok(format!("\u{2733} Working ({frame}s)\n"))
        });
        assert!(unknown(spinning, "cannot be told"));
    }

    /// A line of which only the start arrived was not taken, which is a refusal the caller can
    /// act on: the terminal holds it until interrupted.
    #[test]
    fn a_line_cut_short_is_refused() {
        let long = format!("{}end", "x".repeat(1100));
        let echoed = format!("$ cat\n{}\n", "x".repeat(1023));
        let cut = confirm("p1", &long, "$ cat\n", |_| Ok(echoed.clone()));
        assert!(refused(cut, "only its start shows"));
    }

    /// A pane that could not be read back says nothing about the send, which may have landed.
    #[test]
    fn a_pane_that_cannot_be_read_back_is_unknown() {
        let unreadable = confirm("p1", "hello", "$ ", |_| Err("gone".to_string()));
        assert!(unknown(unreadable, "failed: gone"));
    }

    /// The case that made this compare: an approval prompt that ignored a pasted `p` was
    /// confirmed, because its own `(p)` was already on screen.
    #[test]
    fn text_that_was_already_on_the_pane_confirms_only_once_it_shows_again() {
        let prompt = "2. Yes, and don't ask again (p)\n3. No (esc)\n";
        let ignored = confirm("p1", "p", prompt, |_| Ok(prompt.to_string()));
        assert!(refused(ignored, "--key"));
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

    /// A `y` sent to an agent that then prints scrolls copies of `y` off the top of the rows read
    /// as fast as it adds them, so its count does not rise; the pane changing is what shows it
    /// arrived.
    #[test]
    fn short_text_already_on_the_pane_is_confirmed_by_the_pane_changing() {
        let before = "yes\nyours\nContinue? (y/n)\n";
        let after = "yours\nContinue? (y/n) y\nok\n";
        assert_eq!(confirm("p1", "y", before, |_| Ok(after.to_string())), Ok(()));
    }

    /// Keys have no text to find, so what shows they arrived is the pane changing at all.
    #[test]
    fn keys_alone_are_confirmed_by_the_pane_changing() {
        let dialog = "\u{276f} No, exit\n  Yes, I trust this folder\n";
        let moved = "  No, exit\n\u{276f} Yes, I trust this folder\n";
        assert_eq!(confirm("p1", "", dialog, |_| Ok(moved.to_string())), Ok(()));
        let unchanged = confirm("p1", "", dialog, |_| Ok(dialog.to_string()));
        assert!(refused(unchanged, "nothing on it changed"));
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
