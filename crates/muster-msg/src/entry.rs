use serde::{Deserialize, Serialize};

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
