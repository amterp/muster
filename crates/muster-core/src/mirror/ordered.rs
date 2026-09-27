//! A map that iterates in the order its entries arrived, rather than the order its ids sort.
//!
//! The mirror keys everything by id, and ids sort by their spelling rather than by anything a
//! person means: a window drew ten tabs 1, 10, 11, 2, 3 when ids were numbered, and since
//! the chords name places in that list, they named the wrong panes. Parsing an order out of an
//! id would be a rule invented here about a string documented as opaque, so the order is
//! remembered instead: for a bootstrap it is the order the daemon listed things in, and
//! afterwards the order events announced them.
//!
//! An upsert keeps the place its entry already had, so re-stating a tab does not move it to
//! the end of the list.

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

#[cfg(test)]
mod tests {
    use super::Ordered;

    #[test]
    fn entries_iterate_in_arrival_order_and_keep_their_place_when_restated() {
        let mut held: Ordered<String, u32> =
            ["b", "a", "c"].iter().map(|key| ((*key).to_string(), 0)).collect();
        held.insert("a".to_string(), 1);
        held.remove(&"b".to_string());
        held.insert("b".to_string(), 2);
        let order: Vec<&str> = held.keys().map(String::as_str).collect();
        assert_eq!(order, ["a", "c", "b"], "a restated entry stays, a returning one is new");
    }
}
