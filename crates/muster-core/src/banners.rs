//! The banners Muster draws for messages to the human (MIP-9, section 5).
//!
//! A message that wakes the human raises a banner showing it, with a reply field addressed to
//! its author. One banner stands per group, replaced by the group's next message; it stays until
//! the human answers it, dismisses it or reads the group. Neither answering nor dismissing reads
//! the group: reading moves the human's place to the group's end, which would also read an
//! earlier question from somebody else that the human never saw.
//!
//! A banner holds the message it shows - its number and its author - so a reply goes to that
//! author whatever arrives since. While the human is writing a reply or has picked one of the
//! answers a question offers, or a reply they sent is pending, refused or unknown, the banner is
//! not replaced: the newer message waits behind it, and the banner says who wrote it, until the
//! reply is done with.
//!
//! Pure: the daemon's notices, the entries fetched for them, and what the human did go in, and
//! the stack the shell draws comes out.

use std::collections::BTreeMap;

use crate::attention::{GroupKey, HumanNotice};
use crate::composition::DaemonId;
use crate::conversation;
use crate::markdown::{self, Block};
use crate::posting::{self, Sent};

/// How many lines of a message a banner shows; the rest is in the conversation.
pub const LINES: usize = 8;

/// How many banners are drawn at once; the rest fold into a count.
pub const SHOWN: usize = 3;

/// A message as a banner shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub seq: u64,
    pub author: String,
    pub urgent: bool,
    pub at_ms: u64,
    /// The message's first [`LINES`] lines.
    pub body: Vec<Block>,
    /// Whether the body went on past them.
    pub more: bool,
    /// The answers it offers, which make it a question (MIP-9, section 6).
    pub answers: Vec<String>,
}

impl Message {
    /// Entry `seq` of the group's log, decoded as the conversation decodes it, cut to [`LINES`].
    pub fn of(seq: u64, at_ms: u64, message: &conversation::Message) -> Message {
        let (body, more) = markdown::leading(&message.blocks, LINES);
        Message {
            seq,
            author: message.author.clone(),
            urgent: message.urgent,
            at_ms,
            body,
            more,
            answers: message.answers.clone(),
        }
    }
}

/// What a banner shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    /// The message numbered `seq` is being fetched. Not drawn yet: a fetch from this machine's
    /// daemon takes milliseconds, and a banner that appeared empty and then filled would flicker.
    Loading {
        seq: u64,
        from: Vec<String>,
        count: u64,
    },
    Message(Message),
    /// The fetch failed, so the banner says what the daemon's notice said - how many wait and
    /// from whom - and why the message itself is missing. It takes no reply: its author is a
    /// guess.
    Unloaded {
        seq: u64,
        from: Vec<String>,
        count: u64,
        why: String,
    },
}

impl Shown {
    pub fn seq(&self) -> u64 {
        match self {
            Shown::Loading { seq, .. } | Shown::Unloaded { seq, .. } => *seq,
            Shown::Message(message) => message.seq,
        }
    }

    fn loading(seq: u64, notice: &HumanNotice) -> Shown {
        Shown::Loading { seq, from: notice.from.clone(), count: notice.count }
    }
}

#[derive(Debug)]
struct Standing {
    order: u64,
    shown: Shown,
    /// A newer message, waiting for a held banner to let go.
    newer: Option<Shown>,
    /// The human is writing a reply.
    held: bool,
    /// Which of the message's answers the human picked, to send with the reply.
    picked: Option<usize>,
    sent: Option<Sent>,
    /// Whether the group's daemon still says something waits for the human there. A banner
    /// that is holding a reply outlives the group being read, and goes once it lets go.
    waiting: bool,
}

impl Standing {
    /// Whether the banner is holding a reply the human has not finished with, which keeps it up
    /// and keeps its message in place.
    fn holding(&self) -> bool {
        self.held
            || self.picked.is_some()
            || matches!(
                self.sent,
                Some(Sent::Pending { .. } | Sent::Refused { .. } | Sent::Unknown { .. })
            )
    }
}

/// One banner, as the shell draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner {
    pub group: GroupKey,
    pub shown: Shown,
    pub held: bool,
    /// Which of the shown message's answers the human picked.
    pub picked: Option<usize>,
    pub sent: Option<Sent>,
    /// A newer message waiting behind this banner while it holds a reply.
    pub waiting: Option<Message>,
    /// Whether the banner takes a reply now.
    pub replyable: bool,
}

/// A reply from a banner, ready to post: to the author it shows, with the body composed from the
/// human's note and the answer they picked, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub to: Vec<String>,
    pub body: String,
    /// The question's number and the picked answer's words, for the post's `answering`.
    pub answering: Option<(u64, String)>,
}

/// The banners to draw: at most [`SHOWN`], newest first, and how many more are folded away,
/// with the oldest of them, which the folded count opens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stack {
    pub shown: Vec<Banner>,
    pub folded: usize,
    pub oldest: Option<GroupKey>,
}

/// Every banner standing, and what each group has been answered or dismissed through.
#[derive(Debug)]
pub struct Banners {
    standing: BTreeMap<GroupKey, Standing>,
    settled: BTreeMap<GroupKey, u64>,
    raises: u64,
    allowed: bool,
}

impl Default for Banners {
    fn default() -> Banners {
        Banners { standing: BTreeMap::new(), settled: BTreeMap::new(), raises: 0, allowed: true }
    }
}

impl Banners {
    pub fn new() -> Banners {
        Banners::default()
    }

    /// Whether messages may raise banners: `[notifications] messages` and mute, which take the
    /// operating system's banner away too. Turning them off takes down every banner not holding
    /// a reply.
    pub fn notifying(&mut self, allowed: bool) -> bool {
        self.allowed = allowed;
        if allowed {
            return false;
        }
        let before = self.standing.len();
        self.standing.retain(|_, standing| standing.holding());
        before != self.standing.len()
    }

    /// What a group's daemon now says waits for the human there. `looking` is the human
    /// reading the group already, which raises nothing.
    ///
    /// Returns the number of a message to fetch from the group's log, for [`Banners::fetched`].
    pub fn noticed(
        &mut self,
        group: &GroupKey,
        notice: Option<&HumanNotice>,
        looking: bool,
    ) -> Option<u64> {
        let waiting = notice.filter(|notice| notice.count > 0 && !looking);
        let Some(notice) = waiting else {
            if let Some(standing) = self.standing.get_mut(group) {
                if standing.holding() {
                    standing.waiting = false;
                    standing.newer = None;
                } else {
                    self.standing.remove(group);
                }
            }
            return None;
        };
        let last = notice.last;
        if self.settled.get(group).is_some_and(|settled| *settled >= last) {
            return None;
        }
        let order = self.raises + 1;
        match self.standing.get_mut(group) {
            Some(standing) => {
                standing.waiting = true;
                let newest = standing.newer.as_ref().unwrap_or(&standing.shown).seq();
                if newest >= last {
                    return None;
                }
                if standing.holding() {
                    standing.newer = Some(Shown::loading(last, notice));
                } else if self.allowed {
                    self.raises = order;
                    *standing = Standing {
                        order,
                        shown: Shown::loading(last, notice),
                        newer: None,
                        held: false,
                        picked: None,
                        sent: None,
                        waiting: true,
                    };
                } else {
                    self.standing.remove(group);
                    return None;
                }
            }
            None if self.allowed => {
                self.raises = order;
                self.standing.insert(
                    group.clone(),
                    Standing {
                        order,
                        shown: Shown::loading(last, notice),
                        newer: None,
                        held: false,
                        picked: None,
                        sent: None,
                        waiting: true,
                    },
                );
            }
            None => return None,
        }
        Some(last)
    }

    /// The message numbered `seq` in `group`, fetched for a banner, or why it could not be.
    /// Returns whether anything drawn changed.
    pub fn fetched(
        &mut self,
        group: &GroupKey,
        seq: u64,
        fetched: Result<Message, String>,
    ) -> bool {
        let Some(standing) = self.standing.get_mut(group) else { return false };
        let slot = if matches!(standing.shown, Shown::Loading { seq: s, .. } if s == seq) {
            &mut standing.shown
        } else if let Some(newer @ Shown::Loading { .. }) = &mut standing.newer
            && newer.seq() == seq
        {
            newer
        } else {
            return false;
        };
        let Shown::Loading { from, count, .. } = slot else { return false };
        *slot = match fetched {
            Ok(message) => Shown::Message(message),
            Err(why) => Shown::Unloaded { seq, from: std::mem::take(from), count: *count, why },
        };
        true
    }

    /// The human started or stopped writing a reply in the banner showing `seq`.
    pub fn hold(&mut self, group: &GroupKey, seq: u64, held: bool) -> Result<bool, String> {
        let standing = self.showing(group, seq)?;
        if standing.held == held {
            return Ok(false);
        }
        standing.held = held;
        if !held {
            self.let_go(group);
        }
        Ok(true)
    }

    /// The human picked the answer numbered `answer` (from 0) of the question the banner showing
    /// `seq` asks, or with `None` took their pick back. A pick is sent with the reply, not on its
    /// own, so the human can add a note; it holds the banner as writing does.
    pub fn pick(
        &mut self,
        group: &GroupKey,
        seq: u64,
        answer: Option<usize>,
    ) -> Result<bool, String> {
        let standing = self.showing(group, seq)?;
        match &standing.sent {
            Some(Sent::Pending { .. }) => {
                return Err("a reply from this banner is still being sent, so its answer cannot \
                            change now"
                    .to_string());
            }
            Some(Sent::Posted { .. }) => {
                return Err(
                    "this banner's reply was sent already, so nothing was picked".to_string()
                );
            }
            _ => {}
        }
        if let Some(answer) = answer {
            let offered = match &standing.shown {
                Shown::Message(message) => message.answers.len(),
                _ => 0,
            };
            if answer >= offered {
                return Err(format!(
                    "the message the banner for {} shows offers {offered} answers, so answer {} \
                     is none of them; nothing was picked",
                    group.group,
                    answer + 1
                ));
            }
        }
        if standing.picked == answer {
            return Ok(false);
        }
        standing.picked = answer;
        if answer.is_none() {
            self.let_go(group);
        }
        Ok(true)
    }

    /// The human sent `note` from the banner showing `seq`: the reply to post, or why it cannot
    /// be sent. With an answer picked, the body starts with it and the note may be empty. The
    /// banner then says it is pending until [`Banners::replied`].
    pub fn reply(&mut self, group: &GroupKey, seq: u64, note: &str) -> Result<Reply, String> {
        let standing = self.showing(group, seq)?;
        let Shown::Message(message) = &standing.shown else {
            return Err(format!(
                "the banner for {} could not load its message, so whom a reply would go to is \
                 not known and nothing was sent; open the conversation to answer there",
                group.group
            ));
        };
        if posting::is_muster(&message.author) {
            return Err(format!(
                "{} is Muster itself, which nobody can address, so nothing was sent",
                message.author
            ));
        }
        let picked = standing.picked.and_then(|answer| message.answers.get(answer));
        if note.trim().is_empty() && picked.is_none() {
            return Err("the reply is empty, so nothing was sent".to_string());
        }
        if matches!(standing.sent, Some(Sent::Pending { .. })) {
            return Err(
                "a reply from this banner is still being sent; it was not sent again".to_string()
            );
        }
        let reply = Reply {
            to: vec![message.author.clone()],
            body: picked
                .map_or_else(|| note.to_string(), |answer| posting::answer_body(answer, note)),
            answering: picked.map(|answer| (seq, answer.clone())),
        };
        standing.sent = Some(Sent::Pending { text: note.to_string() });
        Ok(reply)
    }

    /// What the reply sent from the banner showing `seq` came to. A posted one answers the
    /// message, so the banner says whom it reached until a dismiss or the group's next message;
    /// a refused or unknown one stays in the banner with the words the human wrote and the
    /// answer they picked, not the body composed from them.
    pub fn replied(&mut self, group: &GroupKey, seq: u64, sent: Sent) -> bool {
        let Ok(standing) = self.showing(group, seq) else { return false };
        let note = standing.sent.as_ref().and_then(Sent::text).unwrap_or_default().to_string();
        let posted = matches!(sent, Sent::Posted { .. });
        standing.sent = Some(match sent {
            Sent::Refused { why, .. } => Sent::Refused { text: note, why },
            Sent::Unknown { why, .. } => Sent::Unknown { text: note, why },
            other => other,
        });
        if posted {
            standing.held = false;
            standing.picked = None;
            let settled = self.settled.entry(group.clone()).or_insert(seq);
            *settled = (*settled).max(seq);
            self.let_go(group);
        }
        true
    }

    /// The human dismissed the banner showing `seq`. It does not read the group; a newer
    /// message waiting behind it takes its place.
    pub fn dismiss(&mut self, group: &GroupKey, seq: u64) -> Result<(), String> {
        let standing = self.showing(group, seq)?;
        standing.held = false;
        standing.picked = None;
        standing.sent = None;
        let settled = self.settled.entry(group.clone()).or_insert(seq);
        *settled = (*settled).max(seq);
        let Some(standing) = self.standing.get_mut(group) else { return Ok(()) };
        match standing.newer.take() {
            Some(newer) if standing.waiting => self.promote(group, newer),
            _ => {
                self.standing.remove(group);
            }
        }
        Ok(())
    }

    /// A daemon this window no longer holds: its groups' banners go with it.
    pub fn detached(&mut self, daemon: &DaemonId) -> bool {
        let before = self.standing.len();
        self.standing.retain(|group, _| &group.daemon != daemon);
        self.settled.retain(|group, _| &group.daemon != daemon);
        before != self.standing.len()
    }

    /// The banners to draw. One holding a reply is always among them, however many newer ones
    /// arrived: folded away, the words the human was writing would be out of reach.
    pub fn stack(&self) -> Stack {
        let mut drawn: Vec<(&GroupKey, &Standing)> = self
            .standing
            .iter()
            .filter(|(_, standing)| !matches!(standing.shown, Shown::Loading { .. }))
            .collect();
        drawn.sort_by_key(|(_, standing)| std::cmp::Reverse(standing.order));
        let holding = drawn.iter().filter(|(_, standing)| standing.holding()).count();
        let mut room = SHOWN.saturating_sub(holding);
        let (drawn, folded): (Vec<_>, Vec<_>) = drawn.into_iter().partition(|(_, standing)| {
            if standing.holding() {
                return true;
            }
            let fits = room > 0;
            room = room.saturating_sub(1);
            fits
        });
        Stack {
            shown: drawn.into_iter().map(|(group, standing)| banner(group, standing)).collect(),
            folded: folded.len(),
            oldest: folded.last().map(|(group, _)| (*group).clone()),
        }
    }

    /// The banner for `group`, if it is still showing `seq`: an action from a banner the human
    /// was looking at must not land on a newer message that replaced it meanwhile.
    fn showing(&mut self, group: &GroupKey, seq: u64) -> Result<&mut Standing, String> {
        match self.standing.get_mut(group) {
            Some(standing) if standing.shown.seq() == seq => Ok(standing),
            Some(standing) => Err(format!(
                "the banner for {} now shows message #{}, not #{seq}; nothing was done",
                group.group,
                standing.shown.seq()
            )),
            None => Err(format!(
                "no banner for {} is up, so nothing was done; it may have been answered, \
                 dismissed or read",
                group.group
            )),
        }
    }

    /// A banner that stopped holding a reply: a newer message takes its place, and one whose
    /// group was read meanwhile comes down.
    fn let_go(&mut self, group: &GroupKey) {
        let Some(standing) = self.standing.get_mut(group) else { return };
        if standing.holding() {
            return;
        }
        if let Some(newer) = standing.newer.take() {
            self.promote(group, newer);
        } else if !standing.waiting && !matches!(standing.sent, Some(Sent::Posted { .. })) {
            self.standing.remove(group);
        }
    }

    fn promote(&mut self, group: &GroupKey, shown: Shown) {
        self.raises += 1;
        self.standing.insert(
            group.clone(),
            Standing {
                order: self.raises,
                shown,
                newer: None,
                held: false,
                picked: None,
                sent: None,
                waiting: true,
            },
        );
    }
}

fn banner(group: &GroupKey, standing: &Standing) -> Banner {
    let replyable = match &standing.shown {
        Shown::Message(message) => {
            !posting::is_muster(&message.author)
                && !matches!(standing.sent, Some(Sent::Pending { .. } | Sent::Posted { .. }))
        }
        _ => false,
    };
    let waiting = match &standing.newer {
        Some(Shown::Message(message)) => Some(message.clone()),
        _ => None,
    };
    Banner {
        group: group.clone(),
        shown: standing.shown.clone(),
        held: standing.held,
        picked: standing.picked,
        sent: standing.sent.clone(),
        waiting,
        replyable,
    }
}
