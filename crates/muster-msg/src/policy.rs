use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Refusal;
use crate::names::{HUMAN, check_addressee, is_human};

/// A group's rules (MIP-4, section 8), enforced by the service, since a rule a prompt carries
/// has faded by turn 40 (section 9). Names are participants' names, a member on another machine
/// as `name@machine` in the group's home's name for it; `*` is anyone, and `@human` the human.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// Whom an unaddressed post wakes, by author; `*` is any author, or every member but the
    /// human. Waking the human interrupts a person (section 10), so a ring set does it only by
    /// naming `@human`.
    pub ring: BTreeMap<String, Vec<String>>,
    /// Whom an author may address, by author.
    pub allow: BTreeMap<String, Vec<String>>,
    /// Who may add or remove members and change the policy.
    pub membership: Vec<String>,
    /// Who may post urgently, which reaches an agent mid-turn.
    #[serde(default = "everyone")]
    pub urgent: Vec<String>,
    pub paused: bool,
}

fn everyone() -> Vec<String> {
    vec!["*".to_string()]
}

impl Default for Policy {
    fn default() -> Policy {
        let everyone = || BTreeMap::from([("*".to_string(), vec!["*".to_string()])]);
        Policy {
            ring: BTreeMap::from([("*".to_string(), vec!["*".to_string(), HUMAN.to_string()])]),
            allow: everyone(),
            membership: vec!["*".to_string()],
            urgent: vec!["*".to_string()],
            paused: false,
        }
    }
}

impl Policy {
    /// Whether an unaddressed post by `author` wakes `member`. Never the author itself.
    pub(crate) fn rings(&self, author: &str, member: &str) -> bool {
        let (author, member) = (role(author), role(member));
        author != member
            && by_author(&self.ring, author)
                .iter()
                .any(|each| each == member || (each == "*" && member != HUMAN))
    }

    /// Whether `author` may address `addressee`.
    pub(crate) fn allows(&self, author: &str, addressee: &str) -> bool {
        names(by_author(&self.allow, role(author)), role(addressee))
    }

    /// Whom `author` may address, as the policy spells it.
    pub(crate) fn allowed(&self, author: &str) -> Vec<String> {
        by_author(&self.allow, role(author)).to_vec()
    }

    /// Whether `name` may add or remove members, itself included, and change the policy.
    pub(crate) fn permits(&self, name: &str) -> bool {
        names(&self.membership, role(name))
    }

    /// Whether `author` may post urgently.
    pub(crate) fn urges(&self, author: &str) -> bool {
        names(&self.urgent, role(author))
    }

    /// Every name the policy holds, which must each be a participant's name, here or on
    /// another machine, `*`, or `@human`.
    pub(crate) fn check(&self) -> Result<(), Refusal> {
        let keys = self.ring.keys().chain(self.allow.keys());
        let sets = self.ring.values().chain(self.allow.values()).flatten();
        for name in keys.chain(sets).chain(&self.membership).chain(&self.urgent) {
            if name != "*" {
                check_addressee(name)?;
            }
        }
        Ok(())
    }
}

/// The name a policy knows a participant by: the human on another machine, `@human@laptop`,
/// is `@human`, like `*` a role rather than a participant (MIP-4, section 11).
fn role(name: &str) -> &str {
    if is_human(name) { HUMAN } else { name }
}

/// The set a map gives `author`, or failing that the one it gives `*`.
fn by_author<'a>(map: &'a BTreeMap<String, Vec<String>>, author: &str) -> &'a [String] {
    map.get(author).or_else(|| map.get("*")).map_or(&[], Vec::as_slice)
}

fn names(set: &[String], name: &str) -> bool {
    set.iter().any(|each| each == "*" || each == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_rings_everyone_but_the_author() {
        let policy = Policy::default();
        assert!(policy.rings("a", "b"));
        assert!(policy.rings("a", "@human"));
        assert!(!policy.rings("a", "a"));
    }

    #[test]
    fn a_ring_set_rings_the_human_only_by_name() {
        let policy = directed();
        assert!(policy.rings("director", "builder"));
        assert!(!policy.rings("director", "@human"));
        assert!(policy.rings("@human", "director"));
    }

    #[test]
    fn a_directed_ring_wakes_only_the_director() {
        let policy = Policy {
            ring: BTreeMap::from([
                ("director".to_string(), vec!["*".to_string()]),
                ("*".to_string(), vec!["director".to_string()]),
            ]),
            ..Policy::default()
        };
        assert!(policy.rings("director", "builder"));
        assert!(policy.rings("builder", "director"));
        assert!(!policy.rings("builder", "critic"));
    }

    fn directed() -> Policy {
        let set = |names: &[&str]| names.iter().map(ToString::to_string).collect::<Vec<_>>();
        Policy {
            ring: BTreeMap::from([
                ("director".to_string(), set(&["*"])),
                ("*".to_string(), set(&["director"])),
            ]),
            allow: BTreeMap::from([
                ("director".to_string(), set(&["*"])),
                ("@human".to_string(), set(&["*"])),
                ("*".to_string(), set(&["director", "@human"])),
            ]),
            membership: set(&["director", "@human"]),
            urgent: set(&["director", "@human"]),
            paused: false,
        }
    }

    #[test]
    fn the_human_on_another_machine_is_the_human_to_a_policy() {
        let policy = directed();
        assert!(!policy.rings("director", "@human@laptop"));
        assert!(Policy::default().rings("critic", "@human@laptop"));
        assert!(policy.allows("builder", "@human@laptop"));
        assert!(policy.permits("@human@laptop"));
        assert!(!policy.permits("builder@laptop"));
    }

    #[test]
    fn a_directed_member_may_address_only_the_director_and_the_human() {
        let policy = directed();
        assert!(policy.allows("builder", "director"));
        assert!(policy.allows("builder", "@human"));
        assert!(!policy.allows("builder", "critic"));
        assert!(policy.allows("director", "critic"));
        assert!(policy.allows("@human", "critic"));
    }

    #[test]
    fn only_those_named_in_membership_may_change_it() {
        let policy = directed();
        assert!(policy.permits("director"));
        assert!(policy.permits("@human"));
        assert!(!policy.permits("builder"));
        assert!(Policy::default().permits("builder"));
    }

    #[test]
    fn only_those_named_in_urgent_may_post_urgently() {
        let policy = directed();
        assert!(policy.urges("director"));
        assert!(policy.urges("@human@laptop"));
        assert!(!policy.urges("builder"));
        assert!(Policy::default().urges("builder"));
        assert!(Policy::default().urges("@human"));
    }

    #[test]
    fn a_policy_naming_what_no_participant_could_be_called_is_refused() {
        let mut policy = directed();
        assert_eq!(policy.check(), Ok(()));
        policy.membership.push("critic@devenv".to_string());
        assert_eq!(policy.check(), Ok(()));
        policy.membership.push("two words".to_string());
        assert_eq!(policy.check(), Err(Refusal::BadName { name: "two words".to_string() }));
    }
}
