//! The operation ids a daemon remembers, so that a program that asks again for something it may
//! already have got never gets it twice (MIP-10, section 5).
//!
//! A program names an operation with an id of its choosing, scoped by the client name its Hello
//! gave. The daemon that carries the operation out keeps a record of it for [`RETENTION_MS`],
//! saved with the rest of what it keeps, so the record outlives a reconnect, a handoff and a
//! restart. A repeat of a done operation gets its first result; a repeat of one whose fate is
//! not known is never carried out again.
//!
//! The cap never forgets a record before its time: a store holding [`CAPACITY`] refuses a new
//! operation instead, since forgetting one early would carry out a retry of it twice.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How long a record is kept: a day, long enough for a program's server to restart and for a
/// phone that was offline overnight to ask again.
pub(crate) const RETENTION_MS: u64 = 24 * 60 * 60 * 1000;

/// The most records a store holds.
pub(crate) const CAPACITY: usize = 4096;

/// One remembered operation, as a daemon saves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Record<R> {
    client: String,
    id: String,
    /// What was asked ([`fingerprint`]), so that a reused id is told from a repeat.
    asked: u64,
    /// When it was begun, in milliseconds since 1970: a wall clock, since the record has to
    /// mean the same in a restarted daemon.
    at_ms: u64,
    state: State<R>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State<R> {
    /// Begun and not finished.
    UnderWay,
    Done(R),
    /// It may have been carried out, and nothing can say whether it was.
    Uncertain,
}

/// What a store said of an operation about to be carried out.
#[derive(Debug, PartialEq)]
pub(crate) enum Begun<R> {
    /// Not seen before: it is now under way, and the caller carries it out.
    New,
    /// The same operation, still being carried out.
    UnderWay,
    /// The same operation, carried out before, with what it came to.
    Done(R),
    /// The same operation, whose fate is not known.
    Uncertain,
    /// The id named another operation.
    Reused,
    /// The store is full, and nothing was begun.
    Full,
}

/// The operations one daemon carries out of one kind, such as the panes it makes.
#[derive(Debug)]
pub(crate) struct Operations<R> {
    /// Oldest first.
    records: Vec<Record<R>>,
    capacity: usize,
}

impl<R> Default for Operations<R> {
    fn default() -> Self {
        Operations { records: Vec::new(), capacity: CAPACITY }
    }
}

impl<R: Clone> Operations<R> {
    /// The store a daemon saved. An operation it saved while under way may have been carried out
    /// before that daemon stopped or handed over, which nothing here can know.
    pub(crate) fn from_saved(records: Vec<Record<R>>) -> Self {
        let records = records
            .into_iter()
            .map(|mut record| {
                if matches!(record.state, State::UnderWay) {
                    record.state = State::Uncertain;
                }
                record
            })
            .collect();
        Operations { records, ..Operations::default() }
    }

    /// What a daemon saves of the store.
    pub(crate) fn saved(&self) -> Vec<Record<R>> {
        self.records.clone()
    }

    /// Begins the operation `client` calls `id`, which asks what `asked` fingerprints, unless it
    /// was seen within the retention period.
    pub(crate) fn begin(&mut self, client: &str, id: &str, asked: u64, now_ms: u64) -> Begun<R> {
        self.records.retain(|record| now_ms < record.at_ms.saturating_add(RETENTION_MS));
        if let Some(record) = self.find(client, id) {
            if record.asked != asked {
                return Begun::Reused;
            }
            return match &record.state {
                State::UnderWay => Begun::UnderWay,
                State::Done(result) => Begun::Done(result.clone()),
                State::Uncertain => Begun::Uncertain,
            };
        }
        if self.records.len() >= self.capacity {
            return Begun::Full;
        }
        self.records.push(Record {
            client: client.to_string(),
            id: id.to_string(),
            asked,
            at_ms: now_ms,
            state: State::UnderWay,
        });
        Begun::New
    }

    /// The operation was carried out, and came to `result`.
    pub(crate) fn done(&mut self, client: &str, id: &str, result: R) {
        self.settle(client, id, State::Done(result));
    }

    /// The operation may have been carried out, and nothing can say whether it was.
    pub(crate) fn uncertain(&mut self, client: &str, id: &str) {
        self.settle(client, id, State::Uncertain);
    }

    /// The operation was refused before anything was carried out, so asking again under the same
    /// id may carry it out.
    pub(crate) fn forget(&mut self, client: &str, id: &str) {
        self.records.retain(|record| record.client != client || record.id != id);
    }

    fn settle(&mut self, client: &str, id: &str, state: State<R>) {
        let found =
            self.records.iter_mut().find(|record| record.client == client && record.id == id);
        if let Some(record) = found {
            record.state = state;
        }
    }

    fn find(&self, client: &str, id: &str) -> Option<&Record<R>> {
        self.records.iter().find(|record| record.client == client && record.id == id)
    }
}

/// What a request asks, as a number that is the same for the same bytes in every build: FNV-1a
/// over them. A hash a later build could compute differently would read a repeat handed to it
/// as a reused id.
pub(crate) fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The wall clock, in milliseconds since 1970.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn store() -> Operations<String> {
        Operations::default()
    }

    #[test]
    fn a_repeat_of_a_done_operation_gets_its_first_result() {
        let mut store = store();
        assert_eq!(store.begin("foreman", "a", 1, NOW), Begun::New);
        assert_eq!(store.begin("foreman", "a", 1, NOW), Begun::UnderWay);
        store.done("foreman", "a", "p1".to_string());
        assert_eq!(store.begin("foreman", "a", 1, NOW + 1), Begun::Done("p1".to_string()));
    }

    #[test]
    fn an_id_used_for_another_request_is_reused_whatever_its_state() {
        let mut store = store();
        store.begin("foreman", "a", 1, NOW);
        assert_eq!(store.begin("foreman", "a", 2, NOW), Begun::Reused);
        store.done("foreman", "a", "p1".to_string());
        assert_eq!(store.begin("foreman", "a", 2, NOW), Begun::Reused);
    }

    #[test]
    fn ids_belong_to_the_client_that_chose_them() {
        let mut store = store();
        store.begin("foreman", "1", 1, NOW);
        assert_eq!(store.begin("quartermaster", "1", 2, NOW), Begun::New);
    }

    #[test]
    fn a_forgotten_operation_can_be_begun_again_and_an_uncertain_one_cannot() {
        let mut store = store();
        store.begin("foreman", "a", 1, NOW);
        store.forget("foreman", "a");
        assert_eq!(store.begin("foreman", "a", 1, NOW), Begun::New);
        store.uncertain("foreman", "a");
        assert_eq!(store.begin("foreman", "a", 1, NOW), Begun::Uncertain);
    }

    #[test]
    fn a_record_is_kept_for_a_day_and_no_longer() {
        let mut store = store();
        store.begin("foreman", "a", 1, NOW);
        store.done("foreman", "a", "p1".to_string());
        let last = NOW + RETENTION_MS - 1;
        assert_eq!(store.begin("foreman", "a", 1, last), Begun::Done("p1".to_string()));
        assert_eq!(store.begin("foreman", "a", 1, NOW + RETENTION_MS), Begun::New);
    }

    #[test]
    fn a_full_store_refuses_a_new_operation_and_still_answers_its_earliest() {
        let mut store = Operations { capacity: 3, ..store() };
        for id in ["a", "b", "c"] {
            assert_eq!(store.begin("foreman", id, 1, NOW), Begun::New);
            store.done("foreman", id, format!("made {id}"));
        }
        assert_eq!(store.begin("foreman", "d", 1, NOW), Begun::Full);
        assert_eq!(store.begin("foreman", "a", 1, NOW), Begun::Done("made a".to_string()));
        // Room comes back as records age out, never sooner.
        assert_eq!(store.begin("foreman", "d", 1, NOW + RETENTION_MS), Begun::New);
    }

    #[test]
    fn an_operation_saved_under_way_comes_back_uncertain() {
        let mut store = store();
        store.begin("foreman", "a", 1, NOW);
        store.begin("foreman", "b", 1, NOW);
        store.done("foreman", "b", "p1".to_string());
        let saved = serde_json::to_vec(&store.saved()).unwrap();

        let mut loaded = Operations::<String>::from_saved(serde_json::from_slice(&saved).unwrap());
        assert_eq!(loaded.begin("foreman", "a", 1, NOW), Begun::Uncertain);
        assert_eq!(loaded.begin("foreman", "b", 1, NOW), Begun::Done("p1".to_string()));
    }

    #[test]
    fn a_fingerprint_is_fnv_1a() {
        assert_eq!(fingerprint(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fingerprint(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
