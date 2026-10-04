//! How an agent's harness starts its session again after the pane it ran in was lost to a daemon
//! restart, and which of the arguments it was first started with go along.
//!
//! The arguments are the agent process's own, read from the kernel when its hooks reported the
//! session's id, so a wrapper that started it (`ct -m 2`) has already expanded into the
//! harness's own flags (`--model opus --effort high`). What cannot go along is whatever starts
//! or ends a session of its own - a first prompt, `--print`, another `--resume` - and the
//! manifest names those flags, and the ones that take a value, because only the harness's own
//! grammar tells a flag's value from a prompt.

/// Where a `[session]` resume's command takes the arguments carried, as zero or more words.
pub(crate) const ARGUMENTS_PLACEHOLDER: &str = "{args}";

/// Where it takes the session's id.
pub(crate) const SESSION_PLACEHOLDER: &str = "{session}";

/// A manifest's `[session]` resume: the command, and the flags it needs to know about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Spelling {
    pub(crate) command: Vec<String>,
    /// Flags that start or end a session of their own, and are never carried.
    pub(crate) drops: Vec<String>,
    /// Flags that take the word after them as their value, so that word is not a prompt.
    pub(crate) values: Vec<String>,
}

/// The command that resumes a session, and whether the arguments it was started with went
/// along.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resume {
    pub command: Vec<String>,
    /// Why none of the arguments went along, when they did not: a word in them could be a
    /// prompt or a flag's value, and replaying a prompt would send it again.
    pub uncarried: Option<String>,
}

impl Spelling {
    /// The command resuming `session`, carrying what of `arguments` can be carried.
    pub(crate) fn resume(&self, session: &str, arguments: &[String]) -> Resume {
        let (carried, uncarried) = match self.carried(arguments) {
            Ok(carried) => (carried, None),
            Err(why) => (Vec::new(), Some(why)),
        };
        let mut command = Vec::with_capacity(self.command.len() + carried.len());
        for word in &self.command {
            match word.as_str() {
                ARGUMENTS_PLACEHOLDER => command.extend(carried.iter().cloned()),
                SESSION_PLACEHOLDER => command.push(session.to_string()),
                _ => command.push(word.clone()),
            }
        }
        Resume { command, uncarried }
    }

    /// The arguments less what starts a session of its own, or why they cannot be told apart.
    ///
    /// A flag in `values` keeps the word after it; any other flag is a switch. A word that is
    /// neither a flag nor a value is a prompt when it is the last, which is where a harness
    /// takes its first prompt, and is dropped. Anywhere else it may be a flag's second value or
    /// the value of a flag this manifest does not know takes one, and then nothing is carried.
    fn carried(&self, arguments: &[String]) -> Result<Vec<String>, String> {
        let mut carried = Vec::new();
        let mut words = arguments.iter().enumerate().peekable();
        while let Some((at, word)) = words.next() {
            if word == "--" {
                // Everything after it is the prompt.
                break;
            }
            if !word.starts_with('-') || word == "-" {
                if at + 1 == arguments.len() {
                    break;
                }
                return Err(format!(
                    "`{word}` could be a prompt or a flag's value, and a prompt carried into the \
                     resumed session would be sent again"
                ));
            }
            let (flag, inline) = match word.split_once('=') {
                Some((flag, _)) if flag.starts_with("--") => (flag, true),
                _ => (word.as_str(), false),
            };
            let takes_value = !inline && self.values.iter().any(|value| value == flag);
            let value = if takes_value {
                words.next_if(|(_, next)| !next.starts_with('-')).map(|(_, next)| next)
            } else {
                None
            };
            if self.drops.iter().any(|drop| drop == flag) {
                continue;
            }
            carried.push(word.clone());
            carried.extend(value.cloned());
        }
        Ok(carried)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> Spelling {
        let words = |words: &[&str]| words.iter().map(ToString::to_string).collect();
        Spelling {
            command: words(&["claude", "{args}", "--resume", "{session}"]),
            drops: words(&["-p", "--print", "-c", "--continue", "-r", "--resume", "--session-id"]),
            values: words(&["--model", "--effort", "--permission-mode", "-r", "--resume", "-n"]),
        }
    }

    fn resumed(arguments: &[&str]) -> Resume {
        let arguments: Vec<String> = arguments.iter().map(ToString::to_string).collect();
        claude().resume("s-1", &arguments)
    }

    fn command(arguments: &[&str]) -> String {
        resumed(arguments).command.join(" ")
    }

    /// What a wrapper expanded into is carried whole: `ct -m 2` is `--model opus --effort high`
    /// by the time the harness runs.
    #[test]
    fn the_flags_a_wrapper_expanded_into_are_carried() {
        assert_eq!(
            command(&["--model", "opus", "--effort", "high", "--dangerously-skip-permissions"]),
            "claude --model opus --effort high --dangerously-skip-permissions --resume s-1"
        );
        assert_eq!(command(&[]), "claude --resume s-1");
    }

    /// A first prompt was sent when the session started; carried, it would be sent again.
    #[test]
    fn a_first_prompt_is_not_carried() {
        assert_eq!(
            command(&["--model", "opus", "fix the build"]),
            "claude --model opus --resume s-1"
        );
        assert_eq!(command(&["--verbose", "fix it"]), "claude --verbose --resume s-1");
        assert_eq!(
            command(&["--model", "opus", "--", "-x looks like a flag"]),
            "claude --model opus --resume s-1"
        );
    }

    /// What starts or ends a session of its own goes, with its value: an earlier resume is
    /// replaced rather than repeated, and a print run is not one to come back to.
    #[test]
    fn what_starts_a_session_of_its_own_is_not_carried() {
        assert_eq!(
            command(&["--resume", "old", "--model", "opus"]),
            "claude --model opus --resume s-1"
        );
        assert_eq!(command(&["-p", "--model", "opus"]), "claude --model opus --resume s-1");
        assert_eq!(command(&["--continue", "-n", "worker"]), "claude -n worker --resume s-1");
        assert_eq!(command(&["--resume=old", "--model=opus"]), "claude --model=opus --resume s-1");
    }

    /// A word in the middle that is no known flag's value could be either, so nothing is
    /// carried and the resume says why.
    #[test]
    fn a_word_that_could_be_a_prompt_carries_nothing() {
        let resume = resumed(&["--add-dir", "a", "b", "--model", "opus"]);
        assert_eq!(resume.command.join(" "), "claude --resume s-1");
        assert!(resume.uncarried.is_some_and(|why| why.contains("`a`")));
    }

    /// A flag that takes a value and has none before the next flag keeps going without one.
    #[test]
    fn a_value_flag_with_no_value_takes_none() {
        assert_eq!(command(&["--model", "--verbose"]), "claude --model --verbose --resume s-1");
    }
}
