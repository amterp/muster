//! Which pane has the keyboard of a focused window, told to the panes it moves between.
//!
//! A program can ask to hear when it gains and loses focus (mode 1004) - an editor rereads a
//! file changed while you were elsewhere, a multiplexer passes it on to the pane inside. A pane
//! has focus when the keyboard feeds it and its window has the OS's focus, as a Ghostty surface
//! does. Every pane is told, and the daemon writes the report only to a program that asked, so
//! nothing here needs to know which did.
//!
//! Pure: the seam hands it the two inputs as they change and sends what it answers.

use crate::composition::PaneKey;

/// What has been told, and the two things that decide what should have been.
#[derive(Debug, Default)]
pub struct PaneFocus {
    /// Starts false, like attention's: a window not yet told it is focused focuses nothing.
    window_focused: bool,
    keyboard: Option<PaneKey>,
    told: Option<PaneKey>,
}

impl PaneFocus {
    pub fn new() -> PaneFocus {
        PaneFocus::default()
    }

    /// The window gained or lost the OS's focus. Returns who to tell what, the pane losing
    /// focus first.
    pub fn window_focused(&mut self, focused: bool) -> Vec<(PaneKey, bool)> {
        self.window_focused = focused;
        self.settle()
    }

    /// The keyboard moved to this pane, or to none.
    pub fn keyboard(&mut self, pane: Option<PaneKey>) -> Vec<(PaneKey, bool)> {
        self.keyboard = pane;
        self.settle()
    }

    /// A pane that closed is not told it lost focus: there is no program left to tell, and its
    /// name may come back on a new one.
    pub fn forget(&mut self, pane: &PaneKey) {
        if self.told.as_ref() == Some(pane) {
            self.told = None;
        }
        if self.keyboard.as_ref() == Some(pane) {
            self.keyboard = None;
        }
    }

    fn settle(&mut self) -> Vec<(PaneKey, bool)> {
        let focused = if self.window_focused { self.keyboard.clone() } else { None };
        if focused == self.told {
            return Vec::new();
        }
        let lost = std::mem::replace(&mut self.told, focused.clone());
        lost.map(|pane| (pane, false)).into_iter().chain(focused.map(|pane| (pane, true))).collect()
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

    #[test]
    fn a_pane_has_focus_only_while_its_window_does() {
        let mut focus = PaneFocus::new();
        assert!(focus.keyboard(Some(pane("p1"))).is_empty(), "no window focus yet");
        assert_eq!(focus.window_focused(true), [(pane("p1"), true)]);
        assert_eq!(focus.window_focused(false), [(pane("p1"), false)]);
        assert!(focus.window_focused(false).is_empty(), "told once");
    }

    #[test]
    fn the_keyboard_moving_tells_the_pane_it_left_first() {
        let mut focus = PaneFocus::new();
        focus.window_focused(true);
        focus.keyboard(Some(pane("p1")));
        assert_eq!(focus.keyboard(Some(pane("p2"))), [(pane("p1"), false), (pane("p2"), true)]);
        assert!(focus.keyboard(Some(pane("p2"))).is_empty(), "a republish moves nothing");
        assert_eq!(focus.keyboard(None), [(pane("p2"), false)]);
    }

    #[test]
    fn a_pane_that_closed_is_not_told_it_lost_focus() {
        let mut focus = PaneFocus::new();
        focus.window_focused(true);
        focus.keyboard(Some(pane("p1")));
        focus.forget(&pane("p1"));
        assert_eq!(focus.keyboard(Some(pane("p2"))), [(pane("p2"), true)]);
    }
}
