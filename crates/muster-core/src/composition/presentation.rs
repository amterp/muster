//! What the window is showing of itself, as against what it is showing of a session.
//!
//! Composition answers which daemons are attached, which tab each region shows, and how wide
//! each one is. Whether a list is open beside them is not that: it survives a restart the
//! same way and gets written to the same file, but it describes the window rather than the
//! work, and folding it into composition would make "which tabs was I looking at" a question
//! with a chrome setting inside the answer.
//!
//! Here rather than in the shell for the reason nothing else lives there either. The shell
//! owns no truth, so a bool in a window is a second home for durable state - one no test can
//! reach, no corpus can describe, and no CLI can set. As a value in the core it is written
//! down beside the arrangement, rebindable like every other action, and answerable to a case.
//!
//! Small on purpose. Everything here has to be worth a restart remembering; a panel that
//! opens on a chord and closes when you are done with it is not, and does not belong.
//!
//! Two things, then, and they are kept apart deliberately: [`Presentation`] is the window's
//! own chrome, one answer for the whole window, and [`FontSizes`] is per pane. Both are worth
//! a restart remembering and both go in the same file, but only one of them is a fact about
//! the window - so a map keyed by pane stays outside the record that everything else reads as
//! "how the window looked".

use std::collections::BTreeMap;

use crate::composition::record::PaneKey;

/// A rectangle in whatever coordinates the platform draws windows in.
///
/// Four numbers and no screen. Which display a window was on, and whether that display is still
/// there, is a question only a shell can ask - so the shell reports the screens it has and this
/// answers where the window should open, which keeps the rule here where a case can reach it
/// (`fitted`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    /// How much of the title bar has to be on a screen for the window to count as reachable.
    ///
    /// A window is dragged by its title bar and by nothing else, so "still where you left it"
    /// and "still grabbable" are the same question. The numbers are a strip somebody can put a
    /// pointer on: 120 points wide is several times a pointer, and 24 is about the height of
    /// the bar itself. A window narrower or shorter than that is measured against its own size
    /// instead, so a small window sitting fully on a screen is not dragged to the middle of it
    /// every launch.
    const GRABBABLE_WIDTH: f64 = 120.0;
    const TITLE_BAR: f64 = 24.0;

    /// Where this window should actually open, given the screens the machine has now.
    ///
    /// A saved rectangle is a wish on the same terms as a saved region: the display it was
    /// measured on may be unplugged, smaller, or arranged somewhere else entirely. Reopening a
    /// window at 3400x1400 on a laptop alone would put it mostly past the edge, and a window
    /// whose title bar is off-screen cannot be moved back by the person looking at it.
    ///
    /// So: unchanged when it is still reachable, and otherwise fitted to the screen it has most
    /// in common with - clamped to that screen and centred on it. Centred rather than nudged
    /// back into view, because a window that has to be resized has no position worth preserving:
    /// the arrangement it belonged to is gone with the display.
    ///
    /// No screens reported means no fitting at all. That is a shell that could not enumerate
    /// displays rather than a machine with none, and inventing a rectangle for it would move a
    /// window for a reason that turned out to be a bug here.
    #[must_use]
    pub fn fitted(&self, screens: &[Frame]) -> Frame {
        let Some((first, rest)) = screens.split_first() else { return *self };
        if screens.iter().any(|screen| self.is_grabbable_on(screen)) {
            return *self;
        }

        // The first reported screen wins a tie, which is the main one: a window with nothing in
        // common with any display has to land somewhere, and that is where a person is looking.
        let mut best = first;
        let mut most = self.overlapping_area(first);
        for screen in rest {
            let area = self.overlapping_area(screen);
            if area > most {
                best = screen;
                most = area;
            }
        }

        let width = self.width.min(best.width);
        let height = self.height.min(best.height);
        Frame {
            x: best.x + (best.width - width) / 2.0,
            y: best.y + (best.height - height) / 2.0,
            width,
            height,
        }
    }

    /// Whether enough of the title bar is on this screen to put a pointer on.
    fn is_grabbable_on(&self, screen: &Frame) -> bool {
        // The bar is at the top of the frame, which is the high edge of y in the coordinates
        // macOS reports windows in. Whether another platform agrees is not this rule's problem:
        // a shell reporting upside-down screens would be reporting an upside-down window too.
        let bar = Frame {
            x: self.x,
            y: self.y + (self.height - Frame::TITLE_BAR).max(0.0),
            width: self.width,
            height: self.height.min(Frame::TITLE_BAR),
        };
        let (across, down) = bar.overlap_with(screen);
        across > 0.0
            && down > 0.0
            && across >= Frame::GRABBABLE_WIDTH.min(bar.width)
            && down >= Frame::TITLE_BAR.min(bar.height)
    }

    /// How much of this rectangle lies on a screen, across and down. Zero on either axis is no
    /// overlap at all rather than a line of it.
    fn overlap_with(&self, screen: &Frame) -> (f64, f64) {
        let across =
            ((self.x + self.width).min(screen.x + screen.width) - self.x.max(screen.x)).max(0.0);
        let down =
            ((self.y + self.height).min(screen.y + screen.height) - self.y.max(screen.y)).max(0.0);
        (across, down)
    }

    fn overlapping_area(&self, screen: &Frame) -> f64 {
        let (across, down) = self.overlap_with(screen);
        across * down
    }
}

/// The window's own chrome, as much of it as outlives the process.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Presentation {
    /// Whether the roster is on screen.
    pub sidebar: bool,

    /// Where the window last settled, or nothing for one that never has.
    ///
    /// Nothing is a first launch, a state file written before this key existed, or a file
    /// somebody hand-edited into a rectangle with no size. All three end the same way - the
    /// shell opens the window wherever it would have - so they are one absence rather than
    /// three cases.
    ///
    /// The rectangle written down is always the one the window has when it is *not* in
    /// full-screen. macOS reports a full-screen window's frame as the whole display, and
    /// remembering that would leave somebody who leaves full-screen with a window the size of
    /// their monitor and no way back to the size they had.
    pub frame: Option<Frame>,

    /// Whether the window was in the platform's own full-screen when it was last left.
    ///
    /// Beside the rectangle rather than inside it, because it is not a fact about a rectangle:
    /// on macOS full-screen is a space rather than a size, so what is remembered is both - where
    /// the window goes when it comes out, and that it should not be out.
    pub full_screen: bool,
}

impl Default for Presentation {
    /// Sidebar open, because the list is how a pane nobody is showing gets found at all - and
    /// on a first launch nobody has decided otherwise. And no frame at all, which is a window
    /// that has never been anywhere to come back to.
    fn default() -> Presentation {
        Presentation { sidebar: true, frame: None, full_screen: false }
    }
}

impl Presentation {
    #[must_use]
    pub fn with_sidebar(self, sidebar: bool) -> Presentation {
        Presentation { sidebar, ..self }
    }

    /// Where the window is, and whether it is full-screen, as one answer.
    ///
    /// Together because the shell only ever knows both at once: the rectangle it reports while
    /// full-screen is the one it will come back to, so a setter for either alone would invite a
    /// caller to write half of a pair that has to agree.
    #[must_use]
    pub fn with_frame(self, frame: Option<Frame>, full_screen: bool) -> Presentation {
        Presentation { frame, full_screen, ..self }
    }
}

/// How big the text in each pane is, in points away from what the config file asked for.
///
/// Per pane rather than per window. Muster sized every pane at once until it did not, on the
/// argument that a grid you read at a glance is harder to read with ragged cell sizes - but
/// the grid is fifteen agents doing different jobs, and the one you are reading closely is
/// worth more room than the one you are watching for a colour. Ragged is the price of being
/// able to say which.
///
/// An offset rather than a size, because Muster does not know the size it is offsetting from:
/// `[font] size` is optional, and when it is absent the number is the renderer's own and lives
/// on the far side of the seam. What a chord means - one point bigger than whatever you had -
/// survives not knowing that, and is also what somebody pressing the key means.
///
/// Whole points, on the same terms as `resize_step`: a chord cannot mean half of one.
///
/// Only the panes somebody has sized are in here. Zero is the ordinary answer and it is the
/// absence of an entry, so a window nobody has touched holds an empty map and writes no rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FontSizes {
    offsets: BTreeMap<PaneKey, i32>,
}

impl FontSizes {
    /// How far from the configured size one chord moves, and how far an offset may go.
    ///
    /// The bound is not about legibility - the renderer clamps the size it actually paints, so
    /// a larger offset simply saturates there. It is about what gets written down: an offset
    /// nobody can reach by pressing a key is a number in a state file that can only confuse the
    /// person who finds it.
    pub const STEP: i32 = 1;
    pub const LIMIT: i32 = 64;

    /// How far this pane is from the configured size, which for most panes is nowhere.
    #[must_use]
    pub fn offset(&self, pane: &PaneKey) -> i32 {
        self.offsets.get(pane).copied().unwrap_or_default()
    }

    /// Sizes one pane, and says where it landed.
    ///
    /// Saturated rather than refused. Somebody holding the key down is asking to keep going,
    /// and the honest answer at the end of the range is text that stops growing - not a refusal
    /// for a keystroke whose result they cannot see anyway.
    pub fn set(&mut self, pane: &PaneKey, offset: i32) -> i32 {
        let offset = offset.clamp(-FontSizes::LIMIT, FontSizes::LIMIT);
        // Dropped rather than stored as zero, so that the map is the panes somebody has sized
        // and nothing else. A pane put back to the configured size has nothing to remember, and
        // a row saying so would outlive the pane in the state file.
        if offset == 0 {
            self.offsets.remove(pane);
        } else {
            self.offsets.insert(pane.clone(), offset);
        }
        offset
    }

    /// One press of a chord on one pane, and where it landed.
    pub fn adjust(&mut self, pane: &PaneKey, change: FontSizeChange) -> i32 {
        self.set(pane, change.applied(self.offset(pane)))
    }

    /// Gives a pane the size another one has, for a pane that is about to exist.
    ///
    /// What a split means: the pane you made from a pane you had grown opens at the size you
    /// had grown it to, which is what Ghostty's `window-inherit-font-size` does and what
    /// somebody splitting a pane they can finally read expects.
    pub fn inherit(&mut self, pane: &PaneKey, from: &PaneKey) {
        self.set(pane, self.offset(from));
    }

    /// Forgets every pane a caller no longer vouches for.
    ///
    /// Kept rather than dropped for a daemon that is not answering, on the same terms as a
    /// pane's name: a silent connection is not evidence a pane is gone.
    pub fn retain(&mut self, keep: impl Fn(&PaneKey) -> bool) {
        self.offsets.retain(|pane, _| keep(pane));
    }

    /// Every pane somebody has sized, in a stable order, for whatever writes them down.
    pub fn entries(&self) -> impl Iterator<Item = (&PaneKey, i32)> {
        self.offsets.iter().map(|(pane, offset)| (pane, *offset))
    }
}

impl FromIterator<(PaneKey, i32)> for FontSizes {
    /// Through [`FontSizes::set`], so a file somebody hand-edited to a thousand comes back as
    /// the furthest a chord could have taken it rather than as a thousand.
    fn from_iter<I: IntoIterator<Item = (PaneKey, i32)>>(entries: I) -> FontSizes {
        let mut sizes = FontSizes::default();
        for (pane, offset) in entries {
            sizes.set(&pane, offset);
        }
        sizes
    }
}

/// What one press of a font-size chord asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontSizeChange {
    Larger,
    Smaller,
    /// Back to what the config file said, which is the only way out of a size somebody has
    /// pressed themselves into.
    Reset,
}

impl FontSizeChange {
    pub const READABLE: [&'static str; 3] = ["larger", "smaller", "reset"];

    /// Reads one of those, and nothing else. A chord that dispatched a spelling nobody knows
    /// would be a keystroke that does nothing and says nothing, so the seam refuses it by name.
    pub fn parse(name: &str) -> Option<FontSizeChange> {
        match name {
            "larger" => Some(FontSizeChange::Larger),
            "smaller" => Some(FontSizeChange::Smaller),
            "reset" => Some(FontSizeChange::Reset),
            _ => None,
        }
    }

    /// The offset this change produces, given the one in force.
    #[must_use]
    pub fn applied(self, offset: i32) -> i32 {
        match self {
            FontSizeChange::Larger => offset.saturating_add(FontSizes::STEP),
            FontSizeChange::Smaller => offset.saturating_sub(FontSizes::STEP),
            FontSizeChange::Reset => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FontSizeChange, FontSizes, Frame, PaneKey, Presentation};
    use crate::composition::record::DaemonId;
    use crate::mirror::backend::PaneId;

    fn pane(name: &str) -> PaneKey {
        PaneKey::new(&DaemonId::new("local"), &PaneId::new(name))
    }

    #[test]
    fn a_chord_moves_one_point_in_the_direction_it_names() {
        assert_eq!(FontSizeChange::Larger.applied(0), 1);
        assert_eq!(FontSizeChange::Smaller.applied(0), -1);
        assert_eq!(FontSizeChange::Larger.applied(-1), 0);
    }

    /// Reset means what the config file said, which is offset zero - not the smallest size, and
    /// not the size Muster shipped. It is the only way back out of a size somebody has pressed
    /// themselves into, so getting it wrong strands them there.
    #[test]
    fn reset_goes_back_to_what_the_file_said() {
        assert_eq!(FontSizeChange::Reset.applied(7), 0);
        assert_eq!(FontSizeChange::Reset.applied(-7), 0);
        assert_eq!(FontSizeChange::Reset.applied(0), 0);
    }

    /// Somebody holding the key down is asking to keep going, and the honest answer at the end
    /// of the range is text that stops growing - not a refusal for a keystroke whose result
    /// they cannot see anyway. What the bound protects is the state file: an offset nobody
    /// could have pressed their way to is a number that can only confuse whoever finds it.
    #[test]
    fn holding_the_key_down_saturates_rather_than_running_away() {
        let mut sizes = FontSizes::default();
        for _ in 0..(FontSizes::LIMIT + 50) {
            sizes.adjust(&pane("w1:p1"), FontSizeChange::Larger);
        }
        assert_eq!(sizes.offset(&pane("w1:p1")), FontSizes::LIMIT);

        // And it comes back the same way, rather than being stuck at the top. Twice the range,
        // because it starts at the far end of it.
        for _ in 0..(2 * FontSizes::LIMIT + 50) {
            sizes.adjust(&pane("w1:p1"), FontSizeChange::Smaller);
        }
        assert_eq!(sizes.offset(&pane("w1:p1")), -FontSizes::LIMIT);
    }

    /// The whole of what this action reversed. Sizing one pane used to size the window, so
    /// there was nothing to get wrong here; now a chord that reached its neighbours would be
    /// the old behaviour wearing the new one's name.
    #[test]
    fn sizing_one_pane_leaves_the_rest_where_they_were() {
        let mut sizes = FontSizes::default();
        sizes.adjust(&pane("w1:p1"), FontSizeChange::Larger);
        sizes.adjust(&pane("w1:p1"), FontSizeChange::Larger);
        assert_eq!(sizes.offset(&pane("w1:p1")), 2);
        assert_eq!(sizes.offset(&pane("w1:p2")), 0);

        // And a pane nobody has sized costs nothing to remember, which is what keeps the state
        // file a list of exceptions rather than a row per pane.
        assert_eq!(sizes.entries().count(), 1);
    }

    /// A split opens at the size of the pane it came from, which is Ghostty's own answer
    /// (`window-inherit-font-size`, on by default) and what somebody splitting a pane they can
    /// finally read is asking for.
    #[test]
    fn a_new_pane_takes_the_size_of_the_one_it_came_from() {
        let mut sizes = FontSizes::default();
        sizes.set(&pane("w1:p1"), 3);
        sizes.inherit(&pane("w1:p2"), &pane("w1:p1"));
        assert_eq!(sizes.offset(&pane("w1:p2")), 3);

        // From a pane at the configured size it inherits nothing, and writes nothing down.
        sizes.inherit(&pane("w1:p3"), &pane("w1:p9"));
        assert_eq!(sizes.offset(&pane("w1:p3")), 0);
        assert_eq!(sizes.entries().count(), 2);
    }

    /// A pane put back to the configured size is a pane with nothing to remember. Left in the
    /// map it would outlive the pane in the state file, which is a row nobody can explain.
    #[test]
    fn a_pane_back_at_the_configured_size_is_forgotten() {
        let mut sizes = FontSizes::default();
        sizes.set(&pane("w1:p1"), 5);
        sizes.adjust(&pane("w1:p1"), FontSizeChange::Reset);
        assert_eq!(sizes.offset(&pane("w1:p1")), 0);
        assert_eq!(sizes.entries().count(), 0);
    }

    #[test]
    fn one_knob_does_not_move_the_other() {
        let frame = Frame { x: 10.0, y: 20.0, width: 800.0, height: 600.0 };
        let presentation =
            Presentation::default().with_sidebar(false).with_frame(Some(frame), true);
        assert!(!presentation.sidebar);
        assert_eq!(presentation.frame, Some(frame));
        assert!(presentation.full_screen);

        assert_eq!(presentation.with_sidebar(true).frame, Some(frame));
        assert_eq!(presentation.with_frame(None, false).sidebar, presentation.sidebar);
    }

    #[test]
    fn only_the_three_spellings_the_seam_publishes_are_read() {
        for name in FontSizeChange::READABLE {
            assert!(FontSizeChange::parse(name).is_some(), "{name} is published and unreadable");
        }
        assert!(FontSizeChange::parse("bigger").is_none());
        assert!(FontSizeChange::parse("").is_none());
    }
}
