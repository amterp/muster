//! The panes the keyboard has been on in a window, walked back and forward like a browser's.
//!
//! What the mouse's back and forward buttons walk, and `focus_back`, `focus_forward` and
//! `muster focus --back` with them. Every pane the keyboard lands on is an entry, however it got
//! there - a click, a chord, an arrow step inside a tab, a split taking the keyboard - because
//! a history that kept only some moves would skip panes somebody was just in, under a rule they
//! would have to learn.
//!
//! Pure: the seam says where the keyboard landed and which panes still exist, and does the
//! focusing itself.

use crate::composition::PaneKey;

/// How many panes back the history reaches. A window holds about fifteen, so this is several
/// laps of all of them.
const LIMIT: usize = 50;

#[derive(Debug, Default, Clone)]
pub struct FocusHistory {
    entries: Vec<PaneKey>,
    /// The entry the keyboard is on. Meaningless while `entries` is empty.
    at: usize,
}

impl FocusHistory {
    pub fn new() -> FocusHistory {
        FocusHistory::default()
    }

    /// The keyboard landed on this pane.
    ///
    /// Landing where the history already is records nothing, which covers both a republish
    /// that moved nothing and a walk arriving where it moved the history to. Landing anywhere
    /// else drops whatever was ahead, as a browser does after going back.
    pub fn visited(&mut self, pane: PaneKey) {
        if self.entries.get(self.at) == Some(&pane) {
            return;
        }
        self.entries.truncate(self.at + 1);
        self.entries.push(pane);
        if self.entries.len() > LIMIT {
            self.entries.remove(0);
        }
        self.at = self.entries.len() - 1;
    }

    /// Moves to the nearest earlier pane `live` says is still there, and returns it.
    ///
    /// The ones it passes that are gone are dropped on the way. `None` is nowhere to go, and
    /// leaves the history where it was.
    pub fn back(&mut self, live: impl FnMut(&PaneKey) -> bool) -> Option<PaneKey> {
        let order: Vec<usize> = (0..self.at).rev().collect();
        self.walk(order, live)
    }

    pub fn forward(&mut self, live: impl FnMut(&PaneKey) -> bool) -> Option<PaneKey> {
        let order: Vec<usize> = (self.at + 1..self.entries.len()).collect();
        self.walk(order, live)
    }

    /// A pane closed, so there is nothing to go back to. Its name may come back on a new pane,
    /// which is a different pane and starts with no history.
    pub fn forget(&mut self, pane: &PaneKey) {
        self.keep(|_, entry| entry != pane);
    }

    fn walk(
        &mut self,
        order: Vec<usize>,
        mut live: impl FnMut(&PaneKey) -> bool,
    ) -> Option<PaneKey> {
        let mut gone = Vec::new();
        let target = order.into_iter().find(|&index| {
            let here = live(&self.entries[index]);
            if !here {
                gone.push(index);
            }
            here
        })?;
        self.at = target;
        self.keep(|index, _| !gone.contains(&index));
        Some(self.entries[self.at].clone())
    }

    /// Keeps the entries `keep` says to, with the history still on the entry it was on - or
    /// on the one before it, when that entry went. Neighbors that end up equal are merged,
    /// because going back to the pane the keyboard is already on is a press that does nothing.
    fn keep(&mut self, mut keep: impl FnMut(usize, &PaneKey) -> bool) {
        let mut kept: Vec<PaneKey> = Vec::with_capacity(self.entries.len());
        let mut at = 0;
        for (index, entry) in std::mem::take(&mut self.entries).into_iter().enumerate() {
            if keep(index, &entry) && kept.last() != Some(&entry) {
                kept.push(entry);
            }
            if index == self.at {
                at = kept.len().saturating_sub(1);
            }
        }
        self.entries = kept;
        self.at = at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composition::DaemonId;
    use crate::mirror::backend::PaneId;

    fn pane(name: &str) -> PaneKey {
        PaneKey { daemon: DaemonId::new("local"), pane: PaneId::new(name) }
    }

    fn every(_: &PaneKey) -> bool {
        true
    }

    fn walked(names: &[&str]) -> FocusHistory {
        let mut history = FocusHistory::new();
        for name in names {
            history.visited(pane(name));
        }
        history
    }

    #[test]
    fn back_and_forward_retrace_the_panes_the_keyboard_was_on() {
        let mut history = walked(&["p1", "p2", "p3"]);
        assert_eq!(history.back(every), Some(pane("p2")));
        assert_eq!(history.back(every), Some(pane("p1")));
        assert_eq!(history.back(every), None, "nothing before the first pane");
        assert_eq!(history.forward(every), Some(pane("p2")));
        assert_eq!(history.forward(every), Some(pane("p3")));
        assert_eq!(history.forward(every), None, "nothing after the last pane");
    }

    /// The focus a walk causes lands where the walk moved the history to, and a republish lands
    /// where the keyboard already was. Recording either would make back go nowhere new.
    #[test]
    fn landing_where_the_history_is_records_nothing() {
        let mut history = walked(&["p1", "p2", "p3"]);
        let went = history.back(every).expect("there is a pane before p3");
        history.visited(went);
        history.visited(pane("p2"));
        assert_eq!(history.forward(every), Some(pane("p3")), "the walk was recorded");
    }

    #[test]
    fn going_somewhere_new_after_going_back_drops_what_was_ahead() {
        let mut history = walked(&["p1", "p2", "p3"]);
        history.back(every);
        history.visited(pane("p4"));
        assert_eq!(history.forward(every), None);
        assert_eq!(history.back(every), Some(pane("p2")));
    }

    #[test]
    fn a_pane_that_closed_is_stepped_over() {
        let mut history = walked(&["p1", "p2", "p3"]);
        history.forget(&pane("p2"));
        assert_eq!(history.back(every), Some(pane("p1")));
        assert_eq!(history.forward(every), Some(pane("p3")));
    }

    /// p1, p2, p1 with p2 gone is p1 twice in a row, and the second press of back would go
    /// to the pane the keyboard is already on.
    #[test]
    fn a_closed_pane_between_two_visits_to_one_pane_leaves_one_entry() {
        let mut history = walked(&["p0", "p1", "p2", "p1"]);
        history.forget(&pane("p2"));
        assert_eq!(history.back(every), Some(pane("p0")));
    }

    #[test]
    fn a_closed_pane_the_keyboard_was_on_leaves_the_history_on_the_one_before() {
        let mut history = walked(&["p1", "p2", "p3"]);
        history.back(every);
        history.forget(&pane("p2"));
        history.visited(pane("p4"));
        assert_eq!(history.back(every), Some(pane("p1")));
    }

    /// A pane can go without its close reaching the history first - its daemon detached, or
    /// the event is still in flight - so the walk asks, and drops what is gone.
    #[test]
    fn a_walk_passes_over_panes_that_are_no_longer_there() {
        let mut history = walked(&["p1", "p2", "p3"]);
        assert_eq!(history.back(|entry| *entry != pane("p2")), Some(pane("p1")));
        assert_eq!(history.forward(every), Some(pane("p3")), "p2 was not dropped");
    }

    #[test]
    fn nowhere_to_go_leaves_the_history_where_it_was() {
        let mut history = walked(&["p1", "p2"]);
        assert_eq!(history.back(|entry| *entry != pane("p1")), None);
        assert_eq!(history.back(every), Some(pane("p1")));
    }

    #[test]
    fn the_oldest_pane_goes_first_once_the_history_is_full() {
        let names: Vec<String> = (0..=LIMIT).map(|n| format!("p{n}")).collect();
        let mut history = FocusHistory::new();
        for name in &names {
            history.visited(pane(name));
        }
        let mut reached = 0;
        while history.back(every).is_some() {
            reached += 1;
        }
        assert_eq!(reached, LIMIT - 1);
    }

    #[test]
    fn an_empty_history_goes_nowhere() {
        let mut history = FocusHistory::new();
        assert_eq!(history.back(every), None);
        assert_eq!(history.forward(every), None);
        history.forget(&pane("p1"));
        history.visited(pane("p1"));
        assert_eq!(history.back(every), None);
    }
}
