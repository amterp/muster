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
pub use log::{Answering, Change, Entry, Log, Message, What};
pub use members::{Header, Liveness, Member, MemberRow, Unread};
pub use thread::{MessageRow, Row, ThreadView};

/// A name kept on another machine, split into the name there and the machine: `review@devenv`.
/// The rule is the daemon's (`muster_msg::split_machine`), which this crate does not depend on:
/// both halves must be there, so `@human+otter`, a pair group with the human, is kept here.
pub fn split_machine(name: &str) -> Option<(&str, &str)> {
    let (base, machine) = name.rsplit_once('@')?;
    (!base.is_empty() && !machine.is_empty()).then_some((base, machine))
}

/// Which of the groups the human is in a name asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Named {
    /// That group, by the name it is kept under here.
    Group(String),
    /// No group the human is in; these are the ones they are in, by name.
    Unknown(Vec<String>),
    /// A bare name that several groups copied from other machines share, by name.
    Several(Vec<String>),
}

/// Which of the groups the human is in, `known`, `name` asks for: that group itself, or the one
/// copied from another machine whose name there it is, as the daemon takes `review` for
/// `review@devenv`. Resolved here rather than left to the daemon, since everything a window
/// keys by the group, what waits for the human there included, uses the name kept here.
pub fn named<'a>(name: &str, known: impl IntoIterator<Item = &'a str>) -> Named {
    let mut known: Vec<&str> = known.into_iter().collect();
    known.sort_unstable();
    if known.contains(&name) {
        return Named::Group(name.to_string());
    }
    let copies: Vec<&str> = known
        .iter()
        .copied()
        .filter(|group| split_machine(group).is_some_and(|(base, _)| base == name))
        .collect();
    match copies.as_slice() {
        [one] => Named::Group((*one).to_string()),
        [] => Named::Unknown(known.into_iter().map(str::to_string).collect()),
        several => Named::Several(several.iter().map(|group| (*group).to_string()).collect()),
    }
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
        posting::shown(name, &self.called)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_the_group_of_that_name() {
        let known = ["review", "review@devenv"];
        assert_eq!(named("review", known), Named::Group("review".into()));
    }

    #[test]
    fn a_bare_name_is_the_one_group_copied_from_another_machine_under_it() {
        let known = ["@human+p2", "review@devenv"];
        assert_eq!(named("review", known), Named::Group("review@devenv".into()));
    }

    #[test]
    fn a_bare_name_two_machines_share_names_both() {
        let known = ["review@devbox", "review@devenv"];
        assert_eq!(
            named("review", known),
            Named::Several(vec!["review@devbox".into(), "review@devenv".into()])
        );
    }

    #[test]
    fn a_name_no_group_has_says_which_there_are() {
        let known = ["review@devenv", "@human+p2"];
        assert_eq!(
            named("revew", known),
            Named::Unknown(vec!["@human+p2".into(), "review@devenv".into()])
        );
    }
}
