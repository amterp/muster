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

    /// Points added to the font size the config file named, or took from the renderer.
    ///
    /// An offset rather than a size, because Muster does not know the size it is offsetting
    /// from: `[font] size` is optional, and when it is absent the number is the renderer's own
    /// and lives on the far side of the seam. What a chord means - one point bigger than
    /// whatever you had - survives not knowing that, and is also what somebody pressing the key
    /// means.
    ///
    /// Whole points, on the same terms as `resize_step`: a chord cannot mean half of one.
    /// Per window rather than per pane, which is where Muster differs from a terminal on
    /// purpose - a grid of fifteen agents is read at a glance, and one with ragged cell sizes
    /// is harder to read than one without.
    pub font_size_offset: i32,

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
    /// on a first launch nobody has decided otherwise. Font size exactly as configured, and no
    /// frame at all, which is a window that has never been anywhere to come back to.
    fn default() -> Presentation {
        Presentation { sidebar: true, font_size_offset: 0, frame: None, full_screen: false }
    }
}

impl Presentation {
    /// How far from the configured size one chord moves, and how far the offset may go.
    ///
    /// The bound is not about legibility - the renderer clamps the size it actually paints, so
    /// a larger offset simply saturates there. It is about what gets written down: an offset
    /// nobody can reach by pressing a key is a number in a state file that can only confuse the
    /// person who finds it.
    pub const FONT_SIZE_STEP: i32 = 1;
    pub const FONT_SIZE_LIMIT: i32 = 64;

    #[must_use]
    pub fn with_sidebar(self, sidebar: bool) -> Presentation {
        Presentation { sidebar, ..self }
    }

    #[must_use]
    pub fn with_font_size_offset(self, offset: i32) -> Presentation {
        Presentation {
            font_size_offset: offset
                .clamp(-Presentation::FONT_SIZE_LIMIT, Presentation::FONT_SIZE_LIMIT),
            ..self
        }
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
            FontSizeChange::Larger => offset.saturating_add(Presentation::FONT_SIZE_STEP),
            FontSizeChange::Smaller => offset.saturating_sub(Presentation::FONT_SIZE_STEP),
            FontSizeChange::Reset => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FontSizeChange, Frame, Presentation};

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
        let mut presentation = Presentation::default();
        for _ in 0..(Presentation::FONT_SIZE_LIMIT + 50) {
            let offset = FontSizeChange::Larger.applied(presentation.font_size_offset);
            presentation = presentation.with_font_size_offset(offset);
        }
        assert_eq!(presentation.font_size_offset, Presentation::FONT_SIZE_LIMIT);

        // And it comes back the same way, rather than being stuck at the top. Twice the range,
        // because it starts at the far end of it.
        for _ in 0..(2 * Presentation::FONT_SIZE_LIMIT + 50) {
            let offset = FontSizeChange::Smaller.applied(presentation.font_size_offset);
            presentation = presentation.with_font_size_offset(offset);
        }
        assert_eq!(presentation.font_size_offset, -Presentation::FONT_SIZE_LIMIT);
    }

    #[test]
    fn one_knob_does_not_move_the_other() {
        let frame = Frame { x: 10.0, y: 20.0, width: 800.0, height: 600.0 };
        let presentation = Presentation::default()
            .with_sidebar(false)
            .with_font_size_offset(4)
            .with_frame(Some(frame), true);
        assert!(!presentation.sidebar);
        assert_eq!(presentation.font_size_offset, 4);
        assert_eq!(presentation.frame, Some(frame));
        assert!(presentation.full_screen);

        assert!(!presentation.with_font_size_offset(0).sidebar);
        assert_eq!(presentation.with_sidebar(true).font_size_offset, 4);
        assert_eq!(presentation.with_sidebar(true).frame, Some(frame));
        assert_eq!(presentation.with_frame(None, false).font_size_offset, 4);
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
