use serde::{Deserialize, Serialize};

use crate::Policy;

/// One line of a group's log. Messages and notices share one sequence, so a log reads in the
/// order things happened; only messages count as unread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    /// Milliseconds since the Unix epoch, from the host's clock.
    pub at_ms: u64,
    #[serde(flatten)]
    pub what: What,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum What {
    Message {
        author: String,
        /// Whom the message is for. Empty means the group's ring set for its author.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        to: Vec<String>,
        body: String,
        /// Its author asked for it to reach its addressees mid-turn, rather than once they are
        /// idle.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        urgent: bool,
    },
    Created {
        by: String,
    },
    Joined {
        who: String,
    },
    Left {
        who: String,
    },
    /// A change to the group itself: its policy set, or the group paused or resumed.
    Changed {
        by: String,
        change: Change,
        /// The policy a `SetPolicy` set, so a replica taking several in one batch rings each
        /// message under the one it was posted under. Absent from entries written before it was
        /// recorded, and from a peer too old to send it: those read as before, under the policy
        /// the batch ends with.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        policy: Option<Box<Policy>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    SetPolicy,
    Paused,
    Resumed,
}

impl Entry {
    /// The author, when this is a message.
    pub fn author(&self) -> Option<&str> {
        match &self.what {
            What::Message { author, .. } => Some(author),
            _ => None,
        }
    }

    /// Who a notice is about, when this is one.
    pub(crate) fn subject(&self) -> Option<&str> {
        match &self.what {
            What::Message { .. } => None,
            What::Created { by } | What::Changed { by, .. } => Some(by),
            What::Joined { who } | What::Left { who } => Some(who),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A log written before entries recorded the policy a change set still reads, and one
    /// recording it reads back as it was written.
    #[test]
    fn a_change_reads_with_or_without_the_policy_it_set() {
        let before = r#"{"seq":3,"at_ms":7,"kind":"changed","by":"a","change":"set_policy"}"#;
        let read: Entry = serde_json::from_str(before).expect("an entry from before reads");
        assert_eq!(
            read.what,
            What::Changed { by: "a".to_string(), change: Change::SetPolicy, policy: None }
        );

        let recorded = Entry {
            seq: 4,
            at_ms: 8,
            what: What::Changed {
                by: "a".to_string(),
                change: Change::SetPolicy,
                policy: Some(Box::new(Policy::default())),
            },
        };
        let line = serde_json::to_string(&recorded).expect("an entry writes");
        assert_eq!(serde_json::from_str::<Entry>(&line).expect("and reads back"), recorded);
    }
}
