//! The operation ids programs name their posts by, so that asking again after a reply was lost
//! never posts twice (MIP-10, section 5).
//!
//! The group's home is the authority: it keeps the id in the post's own log entry, so the log
//! alone says what was posted under it, whatever a crash took from the state beside it, and a
//! deleted group's retired log keeps the ids of its posts. A machine that sent a post to a home
//! elsewhere keeps where it sent it, so a retry goes there again rather than wherever this
//! machine's groups would send a new post by then.

use std::collections::BTreeMap;

use crate::across::Away;
use crate::service::Operation;
use crate::store::{Log, Operated, Sent};
use crate::{Refusal, What};

/// How long an operation id is remembered: a day, long enough for a program's server to restart
/// and for a phone that was offline overnight to ask again.
pub const OPERATION_RETENTION_MS: u64 = 24 * 60 * 60 * 1000;

/// What a program asking again under an operation id asked before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repeat {
    /// It was posted here, as `seq` in `group`.
    Posted { group: String, seq: u64 },
    /// It was sent to the group's home on another machine, which says whether it was posted.
    Sent(Box<Away>),
}

#[derive(Debug, Default)]
pub(crate) struct Operations {
    /// By the program's name and its id.
    posted: BTreeMap<(String, String), Operated>,
    sent: BTreeMap<(String, String), Sent>,
}

impl Operations {
    /// What the logs and the saved state remember: every post's entry that names its
    /// operation, the records retired logs kept, and the posts sent elsewhere.
    pub(crate) fn restore(logs: &BTreeMap<String, Log>, sent: Vec<Sent>) -> Operations {
        let mut operations = Operations::default();
        for (group, log) in logs {
            for operated in &log.operated {
                operations.remember(operated.clone());
            }
            for entry in &log.entries {
                if let What::Message { via: Some(client), operation: Some(operation), .. } =
                    &entry.what
                {
                    operations.remember(Operated {
                        client: client.clone(),
                        id: operation.id.clone(),
                        asked: operation.asked,
                        group: group.clone(),
                        seq: entry.seq,
                        at_ms: entry.at_ms,
                    });
                }
            }
        }
        for sent in sent {
            let key = (sent.operation.client.clone(), sent.operation.id.clone());
            operations.sent.insert(key, sent);
        }
        operations
    }

    /// What `operation` came to, when it was asked within the retention period; refused when the
    /// id was used for something else.
    pub(crate) fn repeat(
        &self,
        operation: &Operation,
        now_ms: u64,
    ) -> Result<Option<Repeat>, Refusal> {
        let key = (operation.client.clone(), operation.id.clone());
        let reused = || Refusal::OperationReused { id: operation.id.clone() };
        if let Some(posted) = self.posted.get(&key).filter(|posted| live(posted.at_ms, now_ms)) {
            if posted.asked != operation.asked {
                return Err(reused());
            }
            return Ok(Some(Repeat::Posted { group: posted.group.clone(), seq: posted.seq }));
        }
        if let Some(sent) = self.sent.get(&key).filter(|sent| live(sent.at_ms, now_ms)) {
            if sent.operation.asked != operation.asked {
                return Err(reused());
            }
            return Ok(Some(Repeat::Sent(Box::new(sent.away.clone()))));
        }
        Ok(None)
    }

    /// `operation` was posted here as `seq` in `group`.
    pub(crate) fn posted(&mut self, operation: &Operation, group: &str, seq: u64, at_ms: u64) {
        self.posted.retain(|_, posted| live(posted.at_ms, at_ms));
        self.remember(Operated {
            client: operation.client.clone(),
            id: operation.id.clone(),
            asked: operation.asked,
            group: group.to_string(),
            seq,
            at_ms,
        });
    }

    /// `operation` is being sent as `away` to the group's home elsewhere.
    pub(crate) fn sending(&mut self, operation: &Operation, away: &Away, now_ms: u64) {
        self.sent.retain(|_, sent| live(sent.at_ms, now_ms));
        let key = (operation.client.clone(), operation.id.clone());
        self.sent.entry(key).or_insert_with(|| Sent {
            operation: operation.clone(),
            at_ms: now_ms,
            away: away.clone(),
        });
    }

    /// The posts made in `group` that a retry may still ask about, for its retired log to keep.
    pub(crate) fn of_group(&self, group: &str, now_ms: u64) -> Vec<Operated> {
        self.posted
            .values()
            .filter(|posted| posted.group == group && live(posted.at_ms, now_ms))
            .cloned()
            .collect()
    }

    /// The posts sent elsewhere, for the saved state.
    pub(crate) fn sent(&self) -> Vec<Sent> {
        self.sent.values().cloned().collect()
    }

    fn remember(&mut self, operated: Operated) {
        let key = (operated.client.clone(), operated.id.clone());
        self.posted.insert(key, operated);
    }
}

fn live(at_ms: u64, now_ms: u64) -> bool {
    now_ms < at_ms.saturating_add(OPERATION_RETENTION_MS)
}
