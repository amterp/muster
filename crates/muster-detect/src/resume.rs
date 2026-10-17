//! How an agent's harness starts its session again after the pane it ran in was lost to a daemon
//! restart, and which of the arguments it was first started with go along.
//!
//! The arguments are the agent process's own, read from the kernel when its hooks reported the
//! session's id, so a wrapper that started it (`ct -m 2`) has already expanded into the
//! harness's own flags (`--model opus --effort high`). What cannot go along is whatever starts
//! or ends a session of its own - a first prompt, `--print`, another `--resume` - and the
//! manifest names those flags, and the ones that take a value, because only the harness's own
//! grammar tells a flag's value from a prompt.
//!
//! A wrapper can name the command itself, in a `MUSTER_RESUME` on the agent's environment, so
//! that what the wrapper sets up - a settings file it deletes once the agent has read it, a
//! proxy in the environment - is set up again: the arguments say what the wrapper passed, never
//! what it did.

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
    /// Flags carried with their value even when the rest cannot be, because the session is not
    /// the same without them: the model it ran on, and how hard it thought.
    pub(crate) keeps: Vec<String>,
}

/// Whether a command's `{args}` has to be there, as a manifest's does, or may be left out, as a
/// wrapper's may, since a wrapper passes its agent the arguments it always does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arguments {
    Required,
    Optional,
}

/// What is wrong with `command` as a resume, if anything: it has to name a program and take the
/// session's id as one whole word, and `{args}` at most once.
pub(crate) fn command_fault(command: &[String], arguments: Arguments) -> Option<String> {
    if command.first().is_none_or(|program| program.trim().is_empty() || program.starts_with('{')) {
        return Some("names no program".to_string());
    }
    let count = |placeholder| command.iter().filter(|word| *word == placeholder).count();
    if count(SESSION_PLACEHOLDER) != 1 {
        return Some(format!("needs exactly one argument that is exactly {SESSION_PLACEHOLDER}"));
    }
    match (arguments, count(ARGUMENTS_PLACEHOLDER)) {
        (Arguments::Required, 1) | (Arguments::Optional, 0 | 1) => {}
        (Arguments::Required, _) => {
            return Some(format!(
                "needs exactly one argument that is exactly {ARGUMENTS_PLACEHOLDER}"
            ));
        }
        (Arguments::Optional, _) => {
            return Some(format!(
                "takes at most one argument that is exactly {ARGUMENTS_PLACEHOLDER}"
            ));
        }
    }
    command
        .iter()
        .any(|word| word.chars().any(char::is_control))
        .then(|| "holds a control character".to_string())
}

/// The command that resumes a session, and whether the arguments it was started with went
/// along.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resume {
    pub command: Vec<String>,
    /// Why the arguments did not go along, when they did not: a word in them could be a prompt
    /// or a flag's value, and replaying a prompt would send it again. Only the flags the
    /// manifest keeps went, then.
    pub uncarried: Option<String>,
    /// Why the command a `MUSTER_RESUME` named was not used, when one was named and was not;
    /// the manifest's is, instead.
    pub ignored: Option<String>,
}

impl Spelling {
    /// The spelling a wrapper named in `MUSTER_RESUME`, words split on whitespace, reading the
    /// arguments it carries by the manifest's flags; or what is wrong with it.
    pub(crate) fn named(named: &str, manifest: &Spelling) -> Result<Spelling, String> {
        let command: Vec<String> = named.split_whitespace().map(str::to_string).collect();
        if let Some(fault) = command_fault(&command, Arguments::Optional) {
            return Err(format!("MUSTER_RESUME {fault}"));
        }
        Ok(Spelling { command, ..manifest.clone() })
    }

    /// The command resuming `session`, carrying what of `arguments` can be carried.
    pub(crate) fn resume(&self, session: &str, arguments: &[String]) -> Resume {
        let (carried, uncarried) = match self.carried(arguments) {
            Ok(carried) => (carried, None),
            Err(why) => (self.kept(arguments), Some(why)),
        };
        let mut command = Vec::with_capacity(self.command.len() + carried.len());
        for word in &self.command {
            match word.as_str() {
                ARGUMENTS_PLACEHOLDER => command.extend(carried.iter().cloned()),
                SESSION_PLACEHOLDER => command.push(session.to_string()),
                _ => command.push(word.clone()),
            }
        }
        Resume { command, uncarried, ignored: None }
    }

    /// The flags in `keeps` and their values, read out of arguments that cannot be read whole:
    /// `--model opus` or `--model=opus`, wherever they stand.
    fn kept(&self, arguments: &[String]) -> Vec<String> {
        let mut kept = Vec::new();
        for (at, word) in arguments.iter().enumerate() {
            if self.keeps.iter().any(|keep| {
                word.strip_prefix(keep.as_str()).is_some_and(|rest| rest.starts_with('='))
            }) {
                kept.push(word.clone());
            } else if self.keeps.contains(word)
                && let Some(value) = arguments.get(at + 1).filter(|value| !value.starts_with('-'))
            {
                kept.extend([word.clone(), value.clone()]);
            }
        }
        kept
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
            keeps: words(&["--model", "--effort"]),
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

    /// A word in the middle that is no known flag's value could be either, so only the flags
    /// the manifest keeps are carried, and the resume says why the rest were not.
    #[test]
    fn a_word_that_could_be_a_prompt_carries_only_what_is_kept() {
        let resume = resumed(&["--add-dir", "a", "b", "--model", "opus", "--effort=high"]);
        assert_eq!(resume.command.join(" "), "claude --model opus --effort=high --resume s-1");
        assert!(resume.uncarried.is_some_and(|why| why.contains("`a`")));
        let bare = resumed(&["--add-dir", "a", "b", "--model", "--verbose"]);
        assert_eq!(bare.command.join(" "), "claude --resume s-1", "a kept flag with no value");
    }

    /// A wrapper's own command replaces the manifest's, and still carries the arguments where it
    /// asks for them, by the manifest's flags.
    #[test]
    fn a_command_a_wrapper_names_is_the_one_resumed() {
        let arguments: Vec<String> = ["--model", "opus", "first prompt"].map(String::from).to_vec();
        let named = Spelling::named("ct -m 2 --resume {session}", &claude()).unwrap();
        assert_eq!(named.resume("s-1", &arguments).command.join(" "), "ct -m 2 --resume s-1");
        let carrying = Spelling::named("  ct  {args} -r {session} ", &claude()).unwrap();
        assert_eq!(carrying.resume("s-1", &arguments).command.join(" "), "ct --model opus -r s-1");
    }

    /// A named command that could not resume the session is refused, saying why.
    #[test]
    fn a_command_a_wrapper_names_has_to_take_the_session() {
        let refused = |named| Spelling::named(named, &claude()).unwrap_err();
        assert_eq!(
            refused("ct --resume"),
            "MUSTER_RESUME needs exactly one argument that is exactly {session}"
        );
        assert_eq!(refused("   "), "MUSTER_RESUME names no program");
        assert_eq!(refused("{session}"), "MUSTER_RESUME names no program");
        assert_eq!(
            refused("ct {args} {args} {session}"),
            "MUSTER_RESUME takes at most one argument that is exactly {args}"
        );
    }

    /// A flag that takes a value and has none before the next flag keeps going without one.
    #[test]
    fn a_value_flag_with_no_value_takes_none() {
        assert_eq!(command(&["--model", "--verbose"]), "claude --model --verbose --resume s-1");
    }
}
