//! A map that iterates in the order the backend stated, rather than the order its ids sort.
//!
//! The mirror keys everything by id, and those ids are opaque strings the backend spells -
//! `w1:t1`, `w1:p2` ([`super::backend`]). Keyed by them a `BTreeMap` iterates
//! lexicographically, and lexicographic order on those strings is *numeric order right up
//! until there are ten of something*: `w1:t10` sorts between `w1:t1` and `w1:t2`. A window
//! with ten tabs drew them 1, 10, 11, 2, 3 - and since ⌘1 to ⌘9 name places in that list,
//! the chords named the wrong panes. Fifteen panes is this project's own reference workload,
//! so the bug sat exactly where the design says to look.
//!
//! **The fix is to stop deriving order from the id at all.** Parsing a digit out of one to
//! sort it numerically would be the obvious repair and it is the wrong one: the ids are
//! documented as opaque, and a backend that spelled them `alpha`/`beta` would be sorted by a
//! rule invented here. The order was never ours to derive - a snapshot hands over its tabs in
//! an order, and collecting them into a map keyed by id threw that away. So this remembers
//! it: entries iterate in the order they first arrived, which for a bootstrap is the order
//! the backend listed them and afterwards is the order events announced them.
//!
//! An upsert keeps the place its entry already had. A backend replays its whole session on
//! every reconnect ([`super::state::Mirror::bootstrap`]), so re-stating a tab must not move
//! it to the end of the list.
//!
//! What this does **not** claim to know is an order somebody rearranged. herdr has a
//! `tab_moved` event and Muster does not read it yet, so a tab dragged elsewhere by another
//! client keeps the place it arrived in. That is a smaller wrongness than the one this
//! replaces, and an honest one: arrival order is a fact, where sorted-id order was an
//! accident.

use std::collections::BTreeMap;

/// A map from id to value that remembers the order its entries first arrived in.
///
/// Keyed by a `BTreeMap` rather than a hash map for the reason the mirror was already using
/// one: two runs of the same code have to produce the same bytes for a log or a corpus case
/// to be diffable, and that is about the *keys*, which are still sorted. Only iteration of
/// values changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ordered<K: Ord, V> {
    held: BTreeMap<K, (u64, V)>,
    /// The place the next new entry takes. Never reused, so removing an entry cannot make a
    /// later one sort before an earlier one.
    next: u64,
}

impl<K: Ord, V> Default for Ordered<K, V> {
    fn default() -> Ordered<K, V> {
        Ordered { held: BTreeMap::new(), next: 0 }
    }
}

impl<K: Ord + Clone, V> Ordered<K, V> {
    /// Adds an entry, or replaces one while leaving it where it was.
    ///
    /// Returns what was there, on the same terms as `BTreeMap::insert`, so a caller can tell
    /// a first arrival from a restatement - which is what decides whether anything is
    /// announced.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if let Some((_, held)) = self.held.get_mut(&key) {
            return Some(std::mem::replace(held, value));
        }
        let place = self.next;
        self.next += 1;
        self.held.insert(key, (place, value));
        None
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.held.get(key).map(|(_, value)| value)
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.held.get_mut(key).map(|(_, value)| value)
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.held.remove(key).map(|(_, value)| value)
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.held.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.held.len()
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// Every value, in the order they arrived.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.ordered().map(|(_, value)| value)
    }

    /// Every key, in the order they arrived, so a caller walking both sees one order.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.ordered().map(|(key, _)| key)
    }

    /// Every entry, in the order they arrived.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.ordered()
    }

    fn ordered(&self) -> impl Iterator<Item = (&K, &V)> {
        let mut entries: Vec<(&K, u64, &V)> =
            self.held.iter().map(|(key, (place, value))| (key, *place, value)).collect();
        entries.sort_by_key(|(_, place, _)| *place);
        entries.into_iter().map(|(key, _, value)| (key, value))
    }
}

/// Built from a sequence, taking that sequence as the order.
///
/// What makes a snapshot work: its lists are the backend's own order, and collecting one
/// keeps it rather than sorting it away.
impl<K: Ord + Clone, V> FromIterator<(K, V)> for Ordered<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(entries: I) -> Ordered<K, V> {
        let mut held = Ordered::default();
        for (key, value) in entries {
            held.insert(key, value);
        }
        held
    }
}
