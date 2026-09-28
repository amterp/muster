use crate::Refusal;

/// The person at the window. A caller that carries no agent identity is the human (MIP-4,
/// section 10); the `@` keeps the name out of reach of any participant's own.
pub const HUMAN: &str = "@human";

const LONGEST: usize = 64;

/// A group's log is a file named `<group>.log`, and a file name is at most 255 bytes.
pub const LONGEST_GROUP: usize = 250;

fn allowed(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
}

pub(crate) fn check_participant(name: &str) -> Result<(), Refusal> {
    if name == HUMAN || (!name.is_empty() && name.len() <= LONGEST && name.chars().all(allowed)) {
        Ok(())
    } else {
        Err(Refusal::BadName { name: name.to_string() })
    }
}

/// Who a post may be addressed to: a participant's name, or one on another machine as
/// `name@machine` (MIP-4, section 11).
pub(crate) fn check_addressee(name: &str) -> Result<(), Refusal> {
    match split_machine(name) {
        Some((base, machine)) if check_participant(base).is_ok() && is_machine(machine) => Ok(()),
        Some(_) => Err(Refusal::BadName { name: name.to_string() }),
        None => check_participant(name),
    }
}

/// Whether `name` can name a machine: the characters a participant's name may hold.
pub fn is_machine(name: &str) -> bool {
    !name.is_empty() && name.len() <= LONGEST && name.chars().all(allowed)
}

/// A name another machine's member or group goes by here, split into its own name and the
/// machine's: `critic@devenv` is `critic` on `devenv`, and `@human@laptop` the human on
/// `laptop`. `@human` alone names no machine.
pub fn split_machine(name: &str) -> Option<(&str, &str)> {
    let (base, machine) = name.rsplit_once('@')?;
    (!base.is_empty() && !machine.is_empty()).then_some((base, machine))
}

/// Whether `name` is the human: this machine's, or another's as `@human@machine`. There is
/// one person, homed where the app runs, so a policy's `@human` means the human on any machine.
pub(crate) fn is_human(name: &str) -> bool {
    name == HUMAN || split_machine(name).is_some_and(|(base, _)| base == HUMAN)
}

/// A group name is a participant name, or several joined by `+` (see [`pair_group`]).
pub(crate) fn check_group(name: &str) -> Result<(), Refusal> {
    let parts_fit = name.split('+').all(|part| check_participant(part).is_ok());
    if name.len() <= LONGEST_GROUP && parts_fit {
        Ok(())
    } else {
        Err(Refusal::BadName { name: name.to_string() })
    }
}

/// What a caller that never gave a name is called: the last part of its working directory,
/// with anything a name cannot hold replaced.
///
/// Parallel sessions tend to sit in directories of their own - one worktree each - so this is
/// usually the name a person would give the agent anyway.
pub fn default_name(directory: Option<&str>) -> String {
    let last = directory.and_then(|path| path.trim_end_matches('/').rsplit('/').next());
    let name: String = last
        .unwrap_or_default()
        .chars()
        .map(|character| if allowed(character) { character } else { '-' })
        .take(LONGEST - 4)
        .collect();
    let name = name.trim_matches('-');
    if name.is_empty() { "agent".to_string() } else { name.to_string() }
}

/// The group of exactly these participants, which a post to them creates when they share none:
/// their names sorted and joined by `+`, so the same set always names the same group.
pub fn pair_group(names: &[&str]) -> String {
    let mut names: Vec<&str> = names.to_vec();
    names.sort_unstable();
    names.dedup();
    names.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_name_is_the_directory_made_safe() {
        assert_eq!(default_name(Some("/Users/a/src/muster-5")), "muster-5");
        assert_eq!(default_name(Some("/Users/a/src/muster-5/")), "muster-5");
        assert_eq!(default_name(Some("/tmp/my project")), "my-project");
        assert_eq!(default_name(Some("/")), "agent");
        assert_eq!(default_name(None), "agent");
    }

    #[test]
    fn a_name_from_another_machine_splits_at_its_last_at() {
        assert_eq!(split_machine("critic@devenv"), Some(("critic", "devenv")));
        assert_eq!(split_machine("@human@laptop"), Some(("@human", "laptop")));
        assert_eq!(split_machine("@human"), None);
        assert_eq!(split_machine("critic"), None);
        assert_eq!(split_machine("@human+builder"), None);
        assert!(check_addressee("critic@devenv").is_ok());
        assert!(check_addressee("@human@laptop").is_ok());
        assert!(check_addressee("critic@").is_err());
        assert!(check_addressee("a b@devenv").is_err());
        assert!(is_human("@human") && is_human("@human@laptop"));
        assert!(!is_human("critic@devenv"));
    }

    #[test]
    fn a_pair_group_is_the_same_whoever_names_it_first() {
        assert_eq!(pair_group(&["critic", "builder"]), "builder+critic");
        assert_eq!(pair_group(&["builder", "critic"]), "builder+critic");
        assert!(check_group("builder+critic").is_ok());
        assert!(check_group("@human+builder").is_ok());
        assert!(check_group("has space").is_err());
        assert!(check_participant("a+b").is_err());
    }
}
