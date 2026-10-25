//! What a post the human sent from a window came to (MIP-9, section 4).
//!
//! A post has four outcomes, and each simpler scheme loses something: counting every failure as
//! unsent sends a reply twice, and counting every finished request as delivered hides a reply
//! the daemon refused. A post carries no key the daemon could use to refuse a second copy, so an
//! unknown one is never sent again; [`settles`] reads the group's log to find out whether it was
//! kept.

/// Where a post the human sent stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    /// Sent, with no answer yet.
    Pending { text: String },
    /// The daemon kept it as `seq`. `said` is whom it reached, in words; `heard` is whether any
    /// live participant did, which is the CLI's exit 6 when it is false.
    Posted { seq: u64, said: String, heard: bool },
    /// The daemon said no, in its own words.
    Refused { text: String, why: String },
    /// The daemon took the request and never answered, so it may have been kept.
    Unknown { text: String, why: String },
}

impl Sent {
    /// The words of a post that is still the human's to send again or edit: a pending, refused
    /// or unknown one.
    pub fn text(&self) -> Option<&str> {
        match self {
            Sent::Pending { text } | Sent::Refused { text, .. } | Sent::Unknown { text, .. } => {
                Some(text)
            }
            Sent::Posted { .. } => None,
        }
    }
}

/// What a post did for one participant it was for, as the daemon answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Woken,
    AlreadyWoken,
    Waiting,
    Gone,
    Deferred,
    NoAgent,
    NoDoorbell,
    Paused,
    Unreachable,
}

/// One participant a post was for, and what it did for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reached {
    pub name: String,
    pub reach: Reach,
}

/// What a post the daemon never answered came to once the group's log showed it was kept.
pub const KEPT_UNANSWERED: &str =
    "kept, though the daemon never answered; whom it woke is not known";

/// The human's name in every group.
pub const HUMAN: &str = "@human";

/// Muster's own name, for what it says in a group. Never an addressee.
pub const MUSTER: &str = "@muster";

/// Whether `name` is the human, on this machine or, as `@human@laptop`, on another.
pub fn is_human(name: &str) -> bool {
    is_named(name, HUMAN)
}

/// Whether `name` is Muster, on this machine or another.
pub fn is_muster(name: &str) -> bool {
    is_named(name, MUSTER)
}

fn is_named(name: &str, base: &str) -> bool {
    name.strip_prefix(base).is_some_and(|rest| rest.is_empty() || rest.starts_with('@'))
}

/// Whether a live participant heard the post: one was woken or will be rung, was already woken,
/// is held for a paused group or an unreachable machine, or is the human, who reads it in a
/// window. Nobody else, and `muster msg post` exits 6.
pub fn heard(reached: &[Reached]) -> bool {
    reached.iter().any(|reached| match reached.reach {
        Reach::Woken
        | Reach::Deferred
        | Reach::AlreadyWoken
        | Reach::Paused
        | Reach::Unreachable => true,
        Reach::Waiting => is_human(&reached.name),
        Reach::Gone | Reach::NoAgent | Reach::NoDoorbell => false,
    })
}

/// Whom a post reached, in the words a window shows under it: "woke otter", or that nobody live
/// heard it.
pub fn said(reached: &[Reached]) -> String {
    if !heard(reached) {
        return "nobody live heard it".to_string();
    }
    let names_of = |wanted: &[Reach]| -> String {
        reached
            .iter()
            .filter(|reached| wanted.contains(&reached.reach))
            .map(|reached| reached.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut parts = Vec::new();
    for (reaches, words) in [
        (&[Reach::Woken, Reach::AlreadyWoken][..], "woke "),
        (&[Reach::Deferred][..], "will ring "),
        (&[Reach::Paused][..], "held while paused for "),
        (&[Reach::Unreachable][..], "machine unreachable for "),
    ] {
        let names = names_of(reaches);
        if !names.is_empty() {
            parts.push(format!("{words}{names}"));
        }
    }
    if parts.is_empty() { "kept for the human".to_string() } else { parts.join("; ") }
}

/// The body of a reply that picks one of a question's offered answers: the answer first, so
/// whoever asked reads it with no new parsing, then the note written with it, if any (MIP-9,
/// section 6). Composed in one place, so no way of replying words it differently.
pub fn answer_body(answer: &str, note: &str) -> String {
    let note = note.trim();
    if note.is_empty() {
        format!("Answer: {answer}")
    } else {
        format!("Answer: {answer}\n\n{note}")
    }
}

/// One message in a group's log, as far as [`settles`] needs it.
#[derive(Debug, Clone, Copy)]
pub struct Logged<'a> {
    pub seq: u64,
    pub by_human: bool,
    pub to: &'a [String],
    pub body: &'a str,
}

/// The number of the entry that shows an unknown post was kept: a message by the human after
/// `after`, to the same recipients in any order, with the same body.
pub fn settles<'a>(
    log: impl IntoIterator<Item = Logged<'a>>,
    after: u64,
    to: &[String],
    body: &str,
) -> Option<u64> {
    let mut wanted: Vec<&str> = to.iter().map(String::as_str).collect();
    wanted.sort_unstable();
    log.into_iter()
        .filter(|entry| entry.seq > after && entry.by_human && entry.body == body)
        .find(|entry| {
            let mut to: Vec<&str> = entry.to.iter().map(String::as_str).collect();
            to.sort_unstable();
            to == wanted
        })
        .map(|entry| entry.seq)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reached(name: &str, reach: Reach) -> Reached {
        Reached { name: name.to_string(), reach }
    }

    #[test]
    fn a_post_waiting_on_agents_alone_is_heard_by_nobody_but_one_for_the_human_is_heard() {
        let unheard = [reached("a", Reach::Waiting), reached("b", Reach::Gone)];
        assert!(!heard(&unheard));
        assert_eq!(said(&unheard), "nobody live heard it");
        assert!(heard(&[reached("@human", Reach::Waiting)]));
        assert!(!heard(&[reached("c", Reach::NoAgent), reached("d", Reach::NoDoorbell)]));
        assert!(heard(&[reached("@human@laptop", Reach::Waiting)]));
        assert!(!heard(&[reached("@humane", Reach::Waiting)]));
    }

    #[test]
    fn whom_a_post_reached_is_said_by_how() {
        let reached = [
            reached("otter", Reach::Woken),
            reached("heron", Reach::AlreadyWoken),
            reached("lynx", Reach::Deferred),
            reached("gone", Reach::Gone),
        ];
        assert_eq!(said(&reached), "woke otter, heron; will ring lynx");
    }

    #[test]
    fn a_reply_picking_an_answer_starts_with_it_above_its_note() {
        assert_eq!(answer_body("Skip it", ""), "Answer: Skip it");
        assert_eq!(answer_body("Skip it", "  \n"), "Answer: Skip it");
        assert_eq!(answer_body("Skip it", " after lunch\n"), "Answer: Skip it\n\nafter lunch");
    }

    #[test]
    fn an_unknown_post_is_settled_by_the_humans_entry_with_its_recipients_and_body() {
        let (otter, both) = (vec!["otter".to_string()], vec!["b".to_string(), "a".to_string()]);
        let log = [
            Logged { seq: 3, by_human: true, to: &otter, body: "yes" },
            Logged { seq: 5, by_human: false, to: &otter, body: "yes" },
            Logged { seq: 6, by_human: true, to: &both, body: "yes" },
            Logged { seq: 7, by_human: true, to: &otter, body: "yes" },
        ];
        assert_eq!(settles(log, 4, &otter, "yes"), Some(7), "not before `after`, not an agent's");
        assert_eq!(settles(log, 4, &["a".to_string(), "b".to_string()], "yes"), Some(6));
        assert_eq!(settles(log, 7, &otter, "yes"), None);
        assert_eq!(settles(log, 0, &otter, "no"), None);
    }
}
