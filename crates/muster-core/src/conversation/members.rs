//! The conversation's members column and its header (MIP-9, section 1).
//!
//! Who is in the group, whether messages wait on each member unread and why, come from the
//! group's daemon (`Who`), which alone can say why a member has not read: whether its pane is at
//! a dialog, its prompt holds something typed, or its program stopped reading. What each agent
//! is doing comes from the window's own mirror, which moves far more often than membership.

use crate::agent_state::AgentState;
use crate::attention::GroupKey;
use crate::composition::PaneKey;
use crate::conversation::Group;
use crate::posting;

/// One member of a group, as its daemon's `Who` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub liveness: Liveness,
    /// The member's pane, when it is an agent in one this window can reach.
    pub pane: Option<PaneKey>,
    /// What waits on it in this group, unread.
    pub unread: Option<Unread>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    Gone,
    Human,
    Unreachable,
}

/// The first message waiting on a member unread, and why it has not read it, in the daemon's
/// words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unread {
    pub first: u64,
    pub from: String,
    pub why: String,
}

/// One row of the members column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRow {
    /// The participant name, which a post addresses.
    pub name: String,
    pub shown: String,
    pub liveness: Liveness,
    pub pane: Option<PaneKey>,
    /// The agent's state in its pane, when the window follows that pane.
    pub state: Option<AgentState>,
    /// What the agent is doing, in the words its row in the agent list uses; empty for none.
    pub doing: String,
    pub unread: Option<Unread>,
    pub you: bool,
}

/// The members column: agents by name, then the human.
pub fn rows(
    group: &Group,
    pane: impl Fn(&PaneKey) -> Option<(AgentState, String)>,
) -> Vec<MemberRow> {
    let mut rows: Vec<MemberRow> = group
        .members
        .iter()
        .flatten()
        .filter(|member| !posting::is_muster(&member.name))
        .map(|member| {
            let (state, doing) = member.pane.as_ref().and_then(&pane).unzip();
            MemberRow {
                name: member.name.clone(),
                shown: group.shown(&member.name),
                liveness: member.liveness,
                pane: member.pane.clone(),
                state,
                doing: doing.unwrap_or_default(),
                unread: member.unread.clone(),
                you: posting::is_human(&member.name),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.you.cmp(&b.you).then_with(|| a.shown.cmp(&b.shown)));
    rows
}

/// What the conversation's header says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub group: String,
    /// The machine that keeps the group, for one this daemon holds a copy of; `None` for one
    /// kept here.
    pub keeper: Option<String>,
    /// How many members it has, once `Who` has answered.
    pub members: Option<usize>,
    pub paused: bool,
    /// The keeper, while it cannot be reached and the copy here may be missing what was posted
    /// there since.
    pub behind: Option<String>,
}

pub fn header(key: &GroupKey, group: &Group) -> Header {
    let keeper = key
        .group
        .rsplit_once('@')
        .map(|(_, machine)| machine)
        .filter(|machine| !machine.is_empty())
        .map(str::to_string);
    Header {
        group: key.group.clone(),
        keeper,
        members: group.members.as_ref().map(Vec::len),
        paused: group.policy.as_ref().is_some_and(|policy| policy.paused),
        behind: group.log.behind.clone(),
    }
}
