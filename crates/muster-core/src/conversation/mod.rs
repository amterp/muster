//! A group's conversation, as a window shows it in place of its panes (MIP-9, sections 1-4).
//!
//! The core keeps one copy of each open group's log, shared by every window showing it, and
//! builds from it what each window draws: the thread's rows, the members column, the header, and
//! the message box's draft. Which messages fold away, where the unread ones begin, whom a reply
//! opens addressed to and what a send came to are decided here, so a shell on any platform only
//! draws them.
//!
//! Pure: the entries a log answered, what `Who` and `Groups` said, and what the human did go in;
//! rows come out. The seam does the asking.

pub mod draft;
pub mod log;
pub mod members;
pub mod thread;

use std::collections::BTreeMap;

use crate::posting;

pub use draft::{Addressed, Completion, Draft, Edit, Edited};
pub use log::{Change, Entry, Log, Message, What};
pub use members::{Header, Liveness, Member, MemberRow, Unread};
pub use thread::{MessageRow, Reading, Row};

/// A name kept on another machine, split into the name there and the machine: `review@devenv`.
/// The rule is the daemon's (`muster_msg::split_machine`), which this crate does not depend on:
/// both halves must be there, so `@human+otter`, a pair group with the human, is kept here.
pub fn split_machine(name: &str) -> Option<(&str, &str)> {
    let (base, machine) = name.rsplit_once('@')?;
    (!base.is_empty() && !machine.is_empty()).then_some((base, machine))
}

/// What the core holds about one group while a window shows its conversation.
#[derive(Debug, Clone, Default)]
pub struct Group {
    pub log: Log,
    /// The group's policy, from `Groups`; `None` until it has answered.
    pub policy: Option<Policy>,
    /// The group's members, from `Who`; `None` until it has answered.
    pub members: Option<Vec<Member>>,
    /// Each pane participant's callsign, by its participant name, from the daemon's answers.
    pub called: BTreeMap<String, String>,
}

impl Group {
    /// What a participant is called in the conversation: "you" for the human, "Muster" for
    /// `@muster`, and a pane's callsign rather than its id.
    pub fn shown(&self, name: &str) -> String {
        if posting::is_human(name) {
            "you".to_string()
        } else if posting::is_muster(name) {
            "Muster".to_string()
        } else {
            self.called.get(name).cloned().unwrap_or_else(|| name.to_string())
        }
    }

    /// Whether `name` is in the group now. Counted as yes until `Who` has answered, so a
    /// conversation that has not heard yet offers Reply rather than taking it away.
    pub fn is_member(&self, name: &str) -> bool {
        self.members.as_ref().is_none_or(|members| members.iter().any(|m| m.name == name))
    }

    /// Whether a post can be addressed to `name`: not Muster, which is never addressed, not the
    /// human, who is the one posting, and still a member, since an author who left is refused.
    pub fn addressable(&self, name: &str) -> bool {
        !posting::is_muster(name) && !posting::is_human(name) && self.is_member(name)
    }

    /// Whether a message woke the human: addressed to them, or unaddressed in a group whose
    /// policy rings the human for its author.
    pub fn woke_human(&self, message: &Message) -> bool {
        message.to.iter().any(|to| posting::is_human(to))
            || (message.to.is_empty()
                && self.policy.as_ref().is_some_and(|policy| policy.rings_human(&message.author)))
    }
}

/// The parts of a group's policy a conversation reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    /// Whom an unaddressed post wakes, by author; `*` is any author.
    pub ring: BTreeMap<String, Vec<String>>,
    pub paused: bool,
}

impl Policy {
    /// Whether an unaddressed post by `author` rings the human. A ring of `*` is every member
    /// but the human, who is woken only by being named.
    pub fn rings_human(&self, author: &str) -> bool {
        self.ring
            .get(author)
            .or_else(|| self.ring.get("*"))
            .is_some_and(|names| names.iter().any(|name| posting::is_human(name)))
    }
}
