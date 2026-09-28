//! A group's transcript: the pane a notification of a message for the human lands on (MIP-4,
//! section 10). It is an ordinary pane running `muster msg log --follow`, so a window finds one
//! by the command it runs, which the pane's daemon keeps, rather than by a record of its own
//! that could disagree with what is on screen.

const BEFORE: &str = "muster msg log --group '";
const AFTER: &str = "' --follow";

/// What a transcript of `group` runs. Quoted, since a group's name may hold `+` and `@`; a name
/// never holds a quote.
pub fn command(group: &str) -> String {
    format!("{BEFORE}{group}{AFTER}")
}

/// The group a pane running `command` is the transcript of, if it is one.
pub fn group_of(command: &str) -> Option<&str> {
    command.strip_prefix(BEFORE)?.strip_suffix(AFTER)
}

/// What a transcript's pane is called when Muster makes one.
pub fn pane_name(group: &str) -> String {
    format!("✉ {group}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transcript_is_known_by_the_command_it_runs() {
        for group in ["review", "@human+builder", "review@devenv"] {
            assert_eq!(group_of(&command(group)), Some(group));
        }
        assert_eq!(group_of("muster msg log --group 'review'"), None);
        assert_eq!(group_of("claude"), None);
    }
}
