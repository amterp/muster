//! The groups the human is in, as the sidebar lists them (MIP-4, Decision 1b): every one on every
//! daemon a window hears the human's messages from, with how many messages there would wake the
//! human and how many of those were addressed to them.
//!
//! Counted by the daemon and held by each backend's mirror; this only puts them in one list, so
//! the shell draws what it is given and the order is decided once, here.

use crate::attention::HumanNotice;
use crate::composition::DaemonId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageGroup {
    pub daemon: DaemonId,
    pub group: String,
    /// Unread messages that would wake the human.
    pub unread: u64,
    /// How many of those were addressed to the human.
    pub to_you: u64,
}

/// Every group held, by name and then by daemon: a list somebody scans for a name, where the
/// same group on two machines sits together.
pub fn listed<'a>(
    held: impl IntoIterator<Item = (&'a DaemonId, &'a String, &'a HumanNotice)>,
) -> Vec<MessageGroup> {
    let mut groups: Vec<MessageGroup> = held
        .into_iter()
        .filter(|(_, _, notice)| notice.listed())
        .map(|(daemon, group, notice)| MessageGroup {
            daemon: daemon.clone(),
            group: group.clone(),
            unread: notice.count,
            to_you: notice.to_you,
        })
        .collect();
    groups.sort_by(|a, b| (&a.group, &a.daemon).cmp(&(&b.group, &b.daemon)));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(count: u64, to_you: u64, member: bool) -> HumanNotice {
        HumanNotice { count, to_you, member, ..HumanNotice::default() }
    }

    #[test]
    fn groups_are_listed_by_name_then_machine_and_a_group_the_human_left_is_not() {
        let (laptop, devenv) = (DaemonId::new("laptop"), DaemonId::new("devenv"));
        let (review, chat, gone) = ("review".to_string(), "chat".to_string(), "gone".to_string());
        let (unread, read, left) = (notice(3, 1, true), notice(0, 0, true), notice(0, 0, false));
        let held =
            [(&laptop, &review, &unread), (&devenv, &review, &read), (&laptop, &chat, &read)];
        let listed = listed(held.into_iter().chain([(&laptop, &gone, &left)]));
        let named: Vec<(&str, &str, u64, u64)> = listed
            .iter()
            .map(|group| (group.group.as_str(), group.daemon.as_str(), group.unread, group.to_you))
            .collect();
        assert_eq!(
            named,
            [("chat", "laptop", 0, 0), ("review", "devenv", 0, 0), ("review", "laptop", 3, 1)]
        );
    }
}
