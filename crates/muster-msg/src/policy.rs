use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A group's rules (MIP-4, section 8). Every group has the permissive default for now; the
/// fields are all here so that a group convened with a policy of its own changes what is
/// enforced, not what is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// Whom an unaddressed post wakes, by author; `*` is any author, or every member.
    pub ring: BTreeMap<String, Vec<String>>,
    /// Whom an author may address, by author.
    pub allow: BTreeMap<String, Vec<String>>,
    /// Who may add or remove members and change the policy.
    pub membership: Vec<String>,
    pub paused: bool,
}

impl Default for Policy {
    fn default() -> Policy {
        let everyone = || BTreeMap::from([("*".to_string(), vec!["*".to_string()])]);
        Policy {
            ring: everyone(),
            allow: everyone(),
            membership: vec!["*".to_string()],
            paused: false,
        }
    }
}

impl Policy {
    /// Whether an unaddressed post by `author` wakes `member`. Never the author itself.
    pub(crate) fn rings(&self, author: &str, member: &str) -> bool {
        if author == member {
            return false;
        }
        let Some(set) = self.ring.get(author).or_else(|| self.ring.get("*")) else {
            return false;
        };
        set.iter().any(|name| name == "*" || name == member)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_rings_everyone_but_the_author() {
        let policy = Policy::default();
        assert!(policy.rings("a", "b"));
        assert!(!policy.rings("a", "a"));
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
}
