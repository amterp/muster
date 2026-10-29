//! One copy of a group's log, shared by every window showing the group.
//!
//! Fetched newest first, a page at a time, and followed from its last entry, so the copy is the
//! entries from some point to the end with nothing missing between. Each message's Markdown is
//! parsed once, as it arrives.

use std::collections::BTreeMap;

use crate::markdown::{self, Block};
use crate::posting::{self, Logged};

/// One entry in a group's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    pub what: What,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    Message(Message),
    Created(String),
    Joined(String),
    Left(String),
    Changed { by: String, change: Change },
}

/// How somebody changed the group itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Policy,
    Paused,
    Resumed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub author: String,
    pub to: Vec<String>,
    pub body: String,
    pub urgent: bool,
    /// The body's Markdown, parsed once.
    pub blocks: Vec<Block>,
    /// The answers it offers, which make it a question (MIP-9, section 6).
    pub answers: Vec<String>,
    /// The question it answers, when it is a reply that picked one of a question's answers.
    pub answering: Option<Answering>,
    /// The program that posted it for its author, which is shown beside the author.
    pub via: Option<String>,
}

impl Message {
    /// A message offering no answers and answering none, which its author posted themselves.
    pub fn new(author: String, to: Vec<String>, body: String, urgent: bool) -> Message {
        let blocks = markdown::parse(&body);
        Message {
            author,
            to,
            body,
            urgent,
            blocks,
            answers: Vec::new(),
            answering: None,
            via: None,
        }
    }
}

/// Which question a reply answers, and the answer it picked, in the question's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answering {
    pub question: u64,
    pub answer: String,
}

/// The entries of a group held so far.
#[derive(Debug, Clone, Default)]
pub struct Log {
    entries: BTreeMap<u64, Entry>,
    earlier: bool,
    /// The number of the last entry the daemon sent, drawn here or not.
    seen: u64,
    /// The machine the group is kept on, when this copy is behind it because that machine
    /// cannot be reached.
    pub behind: Option<String>,
}

impl Log {
    /// Takes a page of entries older than those held, or the first page. `earlier` is whether
    /// the daemon left out entries before it.
    pub fn page(&mut self, entries: Vec<Entry>, earlier: bool) {
        let older = match (self.earliest(), entries.iter().map(|entry| entry.seq).min()) {
            (None, _) => true,
            (Some(held), Some(first)) => first < held,
            (Some(_), None) => false,
        };
        if older {
            self.earlier = earlier;
        }
        for entry in entries {
            self.entries.entry(entry.seq).or_insert(entry);
        }
    }

    /// Takes entries that followed the last one held, and says which were new.
    pub fn followed(&mut self, entries: Vec<Entry>) -> Vec<u64> {
        let mut new = Vec::new();
        for entry in entries {
            let seq = entry.seq;
            if let std::collections::btree_map::Entry::Vacant(vacant) = self.entries.entry(seq) {
                vacant.insert(entry);
                new.push(seq);
            }
        }
        new
    }

    /// Notes that the daemon sent entry `seq`, whether or not it is one this log holds: a kind
    /// of entry a newer daemon adds is left out, and following from before it again would be
    /// answered with it again, at once and forever.
    pub fn seen(&mut self, seq: u64) {
        self.seen = self.seen.max(seq);
    }

    /// The number of the last entry the daemon sent, or 0 for none: where a follow goes on from.
    pub fn head(&self) -> u64 {
        self.entries.last_key_value().map_or(0, |(seq, _)| *seq).max(self.seen)
    }

    pub fn earliest(&self) -> Option<u64> {
        self.entries.first_key_value().map(|(seq, _)| *seq)
    }

    /// Whether the daemon holds entries before the earliest one here.
    pub fn earlier(&self) -> bool {
        self.earlier
    }

    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.entries.values()
    }

    /// The messages held, as [`posting::settles`] reads them.
    pub fn logged(&self) -> impl Iterator<Item = Logged<'_>> {
        self.entries.values().filter_map(|entry| match &entry.what {
            What::Message(message) => Some(Logged {
                seq: entry.seq,
                by_human: posting::is_human(&message.author),
                to: &message.to,
                body: &message.body,
            }),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(seq: u64) -> Entry {
        Entry { seq, at_ms: seq, what: What::Joined(format!("m{seq}")) }
    }

    #[test]
    fn an_older_page_says_whether_there_is_one_before_it_and_a_newer_one_does_not() {
        let mut log = Log::default();
        log.page(vec![joined(5), joined(6)], true);
        assert!(log.earlier());
        log.page(vec![joined(3), joined(4)], false);
        assert!(!log.earlier());
        log.page(vec![joined(6)], true);
        assert!(!log.earlier(), "a page that is not older says nothing about what came before");
        assert_eq!((log.earliest(), log.head()), (Some(3), 6));
    }

    #[test]
    fn an_entry_left_out_still_moves_the_head() {
        let mut log = Log::default();
        log.page(vec![joined(1)], false);
        log.seen(4);
        assert_eq!(log.followed(Vec::new()), Vec::<u64>::new());
        assert_eq!(log.head(), 4);
    }

    #[test]
    fn a_followed_entry_already_held_is_not_new() {
        let mut log = Log::default();
        log.page(vec![joined(1)], false);
        assert_eq!(log.followed(vec![joined(1), joined(2), joined(3)]), vec![2, 3]);
        assert_eq!(log.head(), 3);
    }
}
