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
        /// Answers its author offers, which make it a question (MIP-9, section 6).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        answers: Vec<String>,
        /// The question it answers, and the answer it picks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answering: Option<Answering>,
        /// The program that posted it for its author, by the name the program gave itself
        /// (MIP-10, section 5): attribution, not identity.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        via: Option<String>,
        /// The operation id that program named the post by, kept in the post's own entry so
        /// that the log alone says a retry was posted already. Only in the group's home's log.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation: Option<OperationId>,
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

/// A reply's pick: the number of the question in its group, and the answer it offered that the
/// reply takes, as the question wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answering {
    pub question: u64,
    pub answer: String,
}

/// A program's operation id for a post, and a fingerprint of what it asked, which tells a repeat
/// of the post from another post reusing the id. Scoped by the program's name, `via`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationId {
    pub id: String,
    pub asked: u64,
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

    /// A message written before questions existed reads as one offering no answers, and a
    /// question and its answer read back as they were written.
    #[test]
    fn a_message_reads_with_or_without_answers() {
        let before = r#"{"seq":3,"at_ms":7,"kind":"message","author":"a","body":"hi"}"#;
        let read: Entry = serde_json::from_str(before).expect("a message from before reads");
        let What::Message { answers, answering, .. } = read.what else { panic!("{read:?}") };
        assert_eq!((answers, answering), (Vec::new(), None));

        let reply = Entry {
            seq: 5,
            at_ms: 9,
            what: What::Message {
                author: "@human".to_string(),
                to: vec!["a".to_string()],
                body: "Answer: Skip it".to_string(),
                urgent: false,
                answers: vec!["Again?".to_string(), "No".to_string()],
                answering: Some(Answering { question: 3, answer: "Skip it".to_string() }),
                via: None,
                operation: None,
            },
        };
        let line = serde_json::to_string(&reply).expect("an entry writes");
        assert_eq!(serde_json::from_str::<Entry>(&line).expect("and reads back"), reply);
    }

    /// A post a program made keeps its name and operation id, and a line written before posts
    /// recorded them reads as one no program made.
    #[test]
    fn a_message_reads_with_or_without_the_program_that_posted_it() {
        let before = r#"{"seq":3,"at_ms":7,"kind":"message","author":"a","body":"hi"}"#;
        let read: Entry = serde_json::from_str(before).expect("a message from before reads");
        let What::Message { via, operation, .. } = read.what else { panic!("{read:?}") };
        assert_eq!((via, operation), (None, None));

        let line = r#"{"seq":4,"at_ms":8,"kind":"message","author":"@human","body":"ok","via":"foreman","operation":{"id":"x","asked":9}}"#;
        let read: Entry = serde_json::from_str(line).expect("a program's post reads");
        let What::Message { via, operation, .. } = &read.what else { panic!("{read:?}") };
        assert_eq!(via.as_deref(), Some("foreman"));
        assert_eq!(operation, &Some(OperationId { id: "x".to_string(), asked: 9 }));
        assert_eq!(serde_json::to_string(&read).expect("an entry writes"), line);
    }
}
