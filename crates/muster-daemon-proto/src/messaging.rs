//! How the messaging commands are spelled, in the one place every speaker of them reads it: the
//! CLI names its verbs from these, and the daemon writes them into the wakes and refusals it
//! sends, which tell an agent the command to run next. A rename is an edit here.

/// The command every messaging verb sits under.
pub const COMMAND: &str = "muster";
/// The namespace under `muster` (MIP-4, Decision 3).
pub const NAMESPACE: &str = "msg";

pub const JOIN: &str = "join";
pub const LEAVE: &str = "leave";
pub const WHO: &str = "who";
pub const POST: &str = "post";
pub const READ: &str = "read";
pub const LOG: &str = "log";
pub const WAIT: &str = "wait";

/// Every verb, in the order `--help` lists them.
pub const VERBS: [&str; 7] = [JOIN, LEAVE, WHO, POST, READ, LOG, WAIT];

/// `muster msg <verb>`, followed by `arguments` when there are any.
pub fn command(verb: &str, arguments: &str) -> String {
    let command = format!("{COMMAND} {NAMESPACE} {verb}");
    if arguments.is_empty() { command } else { format!("{command} {arguments}") }
}

/// What a woken agent is told: how many messages wait for it in which group, from whom, and the
/// command that reads them. It carries the range because Claude Code drops a message identical
/// to one it received shortly before, and no body, because only the agent's own read moves its
/// cursor (MIP-4, section 5).
pub fn wake_text(notice: &crate::msg_answer::Notice) -> String {
    let range = if notice.first == notice.last {
        format!("#{}", notice.first)
    } else {
        format!("#{}-{}", notice.first, notice.last)
    };
    let mut parts = vec![format!("{} new ({range})", notice.count)];
    if notice.to_you > 0 {
        parts.push(format!("{} to you", notice.to_you));
    }
    if !notice.from.is_empty() {
        parts.push(format!("from {}", notice.from.join(", ")));
    }
    let read = command(READ, &format!("--group {}", notice.group));
    format!("[{COMMAND}] {}: {}. Read: {read}", notice.group, parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg_answer::Notice;

    #[test]
    fn a_wake_names_the_range_the_senders_and_the_command_that_reads_them() {
        let notice = Notice {
            group: "review".to_string(),
            first: 40,
            last: 42,
            count: 3,
            to_you: 1,
            from: vec!["director".to_string(), "critic".to_string()],
        };
        assert_eq!(
            wake_text(&notice),
            "[muster] review: 3 new (#40-42), 1 to you, from director, critic. \
             Read: muster msg read --group review"
        );
        let one = Notice { first: 7, last: 7, count: 1, to_you: 0, ..notice };
        assert_eq!(
            wake_text(&one),
            "[muster] review: 1 new (#7), from director, critic. \
             Read: muster msg read --group review"
        );
    }
}
