//! What the doorbell reads before it types into a pane (MIP-4, section 6): whether the agent it
//! is ringing is still the pane's foreground, and whether the screen, read now rather than as
//! detection last published it, is that agent at its prompt - idle, or for an urgent wake at
//! work too - and what the prompt holds.
//!
//! Read with no lock held but the pane's own, just before each write, so what is typed answers
//! to the screen as it is, not as it was when a post came in.

use muster_detect::{Agent, Input, Prompt, System};
use muster_vt::Grid;

use crate::detect::Detecting;
use crate::pane::PaneIo;

/// Where a pane's agent stands, for the doorbell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AtPrompt {
    Empty,
    /// Its prompt holds this: a draft, or a ring left unsent.
    Holds(String),
    /// Not at its prompt, and why, for a log line.
    Not(&'static str),
}

/// `at_work` also takes the prompt of an agent at work, which takes what is typed there into
/// the turn it is running.
pub(crate) fn look(io: &PaneIo, agent: &str, detecting: &Detecting, at_work: bool) -> AtPrompt {
    let Some(manifests) = detecting.manifests() else {
        return AtPrompt::Not("detection has not loaded its manifests");
    };
    let agent = Agent::new(agent);
    if !manifests.reads_prompt(&agent) {
        return AtPrompt::Not("its manifest cannot read its prompt");
    }
    // Before the screen: an agent that has exited leaves its last frame above its shell's
    // prompt, which reads as its prompt still.
    let shell = io.shell().and_then(|pid| u32::try_from(pid).ok()).unwrap_or(0);
    let group = io.foreground_group().and_then(|group| u32::try_from(group).ok());
    let found = muster_detect::probe(shell, group, &System, &manifests);
    if found.shell_in_foreground || found.agent.as_ref() != Some(&agent) {
        return AtPrompt::Not("its agent is no longer in the foreground");
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
    let read = manifests.prompt(&agent, input, &typed);
    let read = if at_work && read.is_none() {
        manifests.prompt_at_work(&agent, input, &typed)
    } else {
        read
    };
    match read {
        Some(Prompt::Empty) => AtPrompt::Empty,
        Some(Prompt::Holds(held)) => AtPrompt::Holds(held),
        None => AtPrompt::Not("its screen is not its prompt"),
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
