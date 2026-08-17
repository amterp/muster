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
//! An order somebody rearranged is stated rather than derived, and [`Ordered::reorder`] takes
//! it. herdr's `tab_moved` carries the whole new order for one workspace as a list
//! (`observations/herdr-0.8.0.md` section 21), so there is nothing to compute from a place and
//! an offset - the sequence arrives and is adopted.
//!
//! What is still arrival order is the order *between* workspaces. A reorder names one
//! workspace's tabs, and this permutes only the places those already hold, so another
//! workspace's tabs stay exactly where they were even when the two interleave. Nothing states
//! a cross-workspace order, so nothing here invents one.

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

    /// Puts these keys in this order, among the places they already hold.
    ///
    /// Their current places are collected and handed back out in the stated sequence, so an
    /// entry this was not told about does not move at all - not even relative to the ones that
    /// did. That is the property a payload scoped to one workspace needs: reordering its tabs
    /// must not reshuffle another workspace's tabs that happen to sit between them.
    ///
    /// A key this does not hold is skipped rather than added, because the sequence is an order
    /// and not a census: a tab the mirror has never heard of arrives on its own event, and
    /// inventing an entry from a position would be an entry with no value to put in it.
    ///
    /// Returns whether anything actually moved, so a caller can stay quiet about a reorder that
    /// reordered nothing - which herdr does not send, but a snapshot re-stating the same order
    /// is the same question.
    pub fn reorder(&mut self, order: &[K]) -> bool {
        // Only the keys this holds, so that a place is never handed to a key that has no entry
        // to put it on - which would shift every key after it by one and silently invent an
        // order nobody stated.
        let named: Vec<&K> = order.iter().filter(|key| self.held.contains_key(key)).collect();
        let mut places: Vec<u64> =
            named.iter().filter_map(|key| self.held.get(key).map(|(place, _)| *place)).collect();
        places.sort_unstable();

        let mut moved = false;
        for (key, place) in named.into_iter().zip(places) {
            if let Some((held, _)) = self.held.get_mut(key) {
                moved |= *held != place;
                *held = place;
            }
        }
        moved
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

    /// Values are `()` because every question here is about keys and their places. A value would
    /// be a second thing each assertion could be wrong about.
    fn held(entries: &[&str]) -> Ordered<String, ()> {
        entries.iter().map(|key| ((*key).to_string(), ())).collect()
    }

    fn order(held: &Ordered<String, ()>) -> Vec<&str> {
        held.keys().map(String::as_str).collect()
    }

    fn names(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    #[test]
    fn a_stated_order_is_the_order() {
        let mut tabs = held(&["a", "b", "c"]);
        assert!(tabs.reorder(&names(&["c", "a", "b"])));
        assert_eq!(order(&tabs), vec!["c", "a", "b"]);
    }

    /// Reported as unchanged, because reporting it republishes the whole window and a
    /// subscription replays a session's orders on every reconnect.
    #[test]
    fn restating_the_order_it_already_had_moves_nothing() {
        let mut tabs = held(&["a", "b", "c"]);
        assert!(!tabs.reorder(&names(&["a", "b", "c"])));
        assert_eq!(order(&tabs), vec!["a", "b", "c"]);
    }

    /// The property that makes a payload scoped to one workspace safe to apply. `b` belongs to
    /// nobody in this order and has to come out sitting exactly where it went in - not merely
    /// somewhere between the two, which a naive reassignment would also satisfy.
    #[test]
    fn an_entry_the_order_does_not_name_does_not_move() {
        let mut tabs = held(&["a", "b", "c", "d"]);
        assert!(tabs.reorder(&names(&["d", "c", "a"])));
        assert_eq!(order(&tabs), vec!["d", "b", "c", "a"]);
    }

    /// A key with no entry cannot be given a place, and must not consume one either: taking a
    /// place for it would shift every key after it and produce an order nobody stated.
    #[test]
    fn a_key_nothing_is_held_under_is_skipped_rather_than_placed() {
        let mut tabs = held(&["a", "b", "c"]);
        assert!(tabs.reorder(&names(&["gone", "c", "a", "b"])));
        assert_eq!(order(&tabs), vec!["c", "a", "b"]);
        assert_eq!(tabs.len(), 3, "a name with no entry became one");
    }

    /// Places are never reused, so a reorder after a removal has a gap in the places it is
    /// permuting. It permutes the places that exist rather than the range they span.
    #[test]
    fn a_reorder_after_a_removal_uses_the_places_that_are_left() {
        let mut tabs = held(&["a", "b", "c"]);
        tabs.remove(&"b".to_string());
        assert!(tabs.reorder(&names(&["c", "a"])));
        assert_eq!(order(&tabs), vec!["c", "a"]);
    }
}
