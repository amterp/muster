//! What the doorbell reads before it types into a pane (MIP-4, section 6): whether the agent it
//! is ringing is still the pane's foreground, and whether the screen, read now rather than as
//! detection last published it, is that agent at its prompt - idle, or for an urgent wake at
//! work too - and what the prompt holds.
//!
//! Read with no lock held but the pane's own, just before each write, so what is typed answers
//! to the screen as it is, not as it was when a post came in.

use std::borrow::Cow;

use muster_detect::{Agent, Detection, Input, Prompt, State, System};
use muster_vt::Grid;

use crate::detect::Detecting;
use crate::pane::PaneIo;

/// Where a pane's agent stands, for the doorbell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AtPrompt {
    /// Empty; `at_work` when it was read as the prompt of an agent at work.
    Empty { at_work: bool },
    /// Its prompt holds this: a draft, or a ring left unsent.
    Holds(String),
    /// Not at its prompt, why, and what the rules made of its screen.
    Not(Unseen, Judged),
}

/// What detection made of a screen whose prompt it could not read, for the log: the rule that
/// decided the screen, and the screen's [`shape`]. Never its text, since a screen holds what
/// somebody typed. Empty when no screen was read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Judged {
    pub(crate) rule: Option<String>,
    pub(crate) shape: String,
}

/// How many of a screen's last rows its [`shape`] describes: a prompt box, what is drawn over or
/// under it, and the footer.
const SHAPE_ROWS: usize = 12;

/// The last [`SHAPE_ROWS`] rows of `screen` that hold anything, each as its indent, its first
/// character and its width: `0╭78 0│78 0╰78 0─160 0❯2 2a61`. Enough to see a box over the
/// prompt, a rule drawn below it, or no caret at all; every letter is written `a` and every digit
/// `#`, so nothing typed or printed can be read back from it.
fn shape(screen: &str) -> String {
    let mut rows: Vec<String> = screen
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(SHAPE_ROWS)
        .map(|line| {
            let indent = line.chars().take_while(|ch| ch.is_whitespace()).count();
            let first = match line.chars().find(|ch| !ch.is_whitespace()) {
                Some(ch) if ch.is_alphabetic() => 'a',
                Some(ch) if ch.is_numeric() => '#',
                Some(ch) => ch,
                None => ' ',
            };
            format!("{indent}{first}{}", line.chars().count())
        })
        .collect();
    rows.reverse();
    rows.join(" ")
}

/// Why the doorbell found no prompt to type into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unseen {
    NoManifests,
    Unreadable,
    NotForeground,
    /// A menu, a viewer or a dialog is drawn over its prompt: what, in words, when the rule
    /// that read the screen says (its manifest's `shows`).
    Covered(Option<String>),
    /// Its screen reads as its agent at work, whatever its state says: a screen the rules
    /// misread, or a turn begun since its state was published.
    AtWork,
    /// No rule finds its prompt on its screen: a pane too short to show it, say.
    NoPrompt,
}

impl Unseen {
    pub(crate) fn why(&self) -> Cow<'static, str> {
        match self {
            Unseen::NoManifests => "detection has not loaded its manifests".into(),
            Unseen::Unreadable => "its manifest cannot read its prompt".into(),
            Unseen::NotForeground => "its agent is no longer in the foreground".into(),
            Unseen::Covered(Some(shows)) => format!("{shows} covers its prompt").into(),
            Unseen::Covered(None) => "a menu, a viewer or a dialog covers its prompt".into(),
            Unseen::AtWork => "its screen reads as its agent at work".into(),
            Unseen::NoPrompt => "its prompt is not on its screen".into(),
        }
    }

    /// Why a screen whose prompt could not be read is not one, by what its rules make of it, and
    /// what the rule that decided says the screen `shows`.
    fn of(detection: &Detection, shows: Option<&str>) -> Unseen {
        let covered = || Unseen::Covered(shows.map(str::to_string));
        match detection.state {
            _ if detection.skip_state_update => covered(),
            State::Blocked => covered(),
            State::Working => Unseen::AtWork,
            State::Idle | State::Unknown => Unseen::NoPrompt,
        }
    }
}

/// `at_work` also takes the prompt of an agent at work, which takes what is typed there into
/// the turn it is running.
pub(crate) fn look(io: &PaneIo, agent: &str, detecting: &Detecting, at_work: bool) -> AtPrompt {
    let Some(manifests) = detecting.manifests() else {
        return AtPrompt::Not(Unseen::NoManifests, Judged::default());
    };
    let agent = Agent::new(agent);
    if !manifests.reads_prompt(&agent) {
        return AtPrompt::Not(Unseen::Unreadable, Judged::default());
    }
    // Before the screen: an agent that has exited leaves its last frame above its shell's
    // prompt, which reads as its prompt still.
    let shell = io.shell().and_then(|pid| u32::try_from(pid).ok()).unwrap_or(0);
    let group = io.foreground_group().and_then(|group| u32::try_from(group).ok());
    let found = muster_detect::probe(shell, group, &System, &manifests);
    if found.shell_in_foreground || found.agent.as_ref() != Some(&agent) {
        return AtPrompt::Not(Unseen::NotForeground, Judged::default());
    }
    let (grid, title) = {
        let screen = io.screen();
        let terminal = screen.terminal();
        // Cell by cell, which costs several calls into libghostty a cell: fine once a ring.
        (terminal.viewport(terminal.columns(), terminal.rows()), terminal.title())
    };
    let (drawn, typed) = views(&grid);
    let title = muster_detect::title(&title);
    let input = Input { screen: &drawn, title: &title, progress: "" };
    let (read, at_work) = match manifests.prompt(&agent, input, &typed) {
        None if at_work => (manifests.prompt_at_work(&agent, input, &typed), true),
        read => (read, false),
    };
    match read {
        Some(Prompt::Empty) => AtPrompt::Empty { at_work },
        Some(Prompt::Holds(held)) => AtPrompt::Holds(held),
        None => {
            let detection = manifests.detect(Some(&agent), input);
            let shows = detection.rule.as_deref().and_then(|rule| manifests.shows(&agent, rule));
            let judged = Judged { rule: detection.rule.clone(), shape: shape(&drawn) };
            AtPrompt::Not(Unseen::of(&detection, shows), judged)
        }
    }
}

/// The screen as detection reads it, and the same screen as typed: a line for each row, a
/// character for each character, with trailing blank rows dropped from both.
fn views(grid: &Grid) -> (String, String) {
    let mut drawn: Vec<String> =
        grid.rows.iter().map(|row| row.text().trim_end().to_string()).collect();
    let mut typed: Vec<String> =
        grid.rows.iter().map(|row| row.typed_text().trim_end().to_string()).collect();
    while drawn.last().is_some_and(String::is_empty) {
        drawn.pop();
        typed.pop();
    }
    (muster_detect::screen_text(&drawn.join("\n")), typed.join("\n"))
}

/// Whether what a prompt holds is a ring's text and nothing else. Spacing is not compared: a
/// prompt wraps where the pane's width says, and a wrap can fall at a space.
pub(crate) fn is_only(held: &str, text: &str) -> bool {
    let bare = |text: &str| text.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    bare(held) == bare(text)
}

/// What is typed for `line`: the line, and a space after it when its last word holds an `@`.
/// Codex opens its file search on the word under its cursor once that word holds an `@`, and a
/// Return pressed while the search is open picks a file rather than sending the line - which a
/// ring's Return, pressed a second after its line, meets (docs/observations/codex-0.154.0.md,
/// section 6). A wake ends in its group's name, `@human+builder`, and a space ends the word.
pub(crate) fn as_typed(line: &str) -> String {
    if line.split_whitespace().last().is_some_and(|word| word.contains('@')) {
        format!("{line} ")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screen_with_no_prompt_is_told_by_what_its_rules_read() {
        let read = |state, skip| Detection {
            state,
            visible: true,
            skip_state_update: skip,
            rule: None,
            waiting: None,
        };
        assert_eq!(Unseen::of(&read(State::Unknown, true), None), Unseen::Covered(None), "a menu");
        assert_eq!(
            Unseen::of(&read(State::Blocked, false), None),
            Unseen::Covered(None),
            "a dialog"
        );
        assert_eq!(Unseen::of(&read(State::Working, false), None), Unseen::AtWork);
        assert_eq!(
            Unseen::of(&read(State::Idle, false), None),
            Unseen::NoPrompt,
            "the idle fallback"
        );
    }

    /// A rule that says what its screen shows names it, and one that does not is still a menu,
    /// a viewer or a dialog.
    #[test]
    fn a_covered_prompt_is_named_by_what_its_rule_says_it_shows() {
        let menu = Detection {
            state: State::Unknown,
            visible: false,
            skip_state_update: true,
            rule: None,
            waiting: None,
        };
        let boxed = Unseen::of(&menu, Some("Claude Code's box for a bug report it drafted"));
        assert_eq!(boxed.why(), "Claude Code's box for a bug report it drafted covers its prompt");
        assert_eq!(Unseen::of(&menu, None).why(), "a menu, a viewer or a dialog covers its prompt");
        let working = Detection { state: State::Working, skip_state_update: false, ..menu };
        assert_eq!(Unseen::of(&working, Some("a box")), Unseen::AtWork, "not hidden");
    }

    /// A screen whose prompt could not be read is logged by where its rules, boxes and caret
    /// are, and none of what is printed or typed on it.
    #[test]
    fn a_screen_is_logged_by_its_shape_and_never_its_text() {
        let screen = [
            "⏺ The token is sk-1234, printed",
            "",
            "✦ Heads up · a tip",
            "  1: Learn more   2: Knew this already   0: Dismiss",
            "──────────",
            "❯\u{a0}a secret draft",
            "──────────",
            "  ⏵⏵ bypass permissions on",
        ]
        .join("\n");
        let shaped = shape(&screen);
        assert_eq!(shaped, "0⏺31 0✦18 2#51 0─10 0❯16 0─10 2⏵26");
        for word in ["token", "sk", "1234", "secret", "Heads"] {
            assert!(!shaped.contains(word), "{word} in {shaped}");
        }

        let tall = vec!["text"; SHAPE_ROWS + 5].join("\n");
        assert_eq!(shape(&tall).split(' ').count(), SHAPE_ROWS, "the last rows only");
    }

    #[test]
    fn a_line_ending_in_a_word_with_an_at_is_typed_with_a_space_after_it() {
        assert_eq!(
            as_typed("muster msg read --group @human+builder"),
            "muster msg read --group @human+builder "
        );
        assert_eq!(
            as_typed("muster msg read --group review@devenv"),
            "muster msg read --group review@devenv "
        );
        assert_eq!(as_typed("/rename builder"), "/rename builder");
        assert_eq!(
            as_typed("from @human. Read: muster msg read --group review"),
            "from @human. Read: muster msg read --group review"
        );
    }
}
