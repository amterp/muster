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

/// The window's own chrome, as much of it as outlives the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl Default for Presentation {
    /// Sidebar open, because the list is how a pane nobody is showing gets found at all - and
    /// on a first launch nobody has decided otherwise. Font size exactly as configured.
    fn default() -> Presentation {
        Presentation { sidebar: true, font_size_offset: 0 }
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
    use super::{FontSizeChange, Presentation};

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
        let presentation = Presentation::default().with_sidebar(false).with_font_size_offset(4);
        assert!(!presentation.sidebar);
        assert_eq!(presentation.font_size_offset, 4);
        assert!(!presentation.with_font_size_offset(0).sidebar);
        assert_eq!(presentation.with_sidebar(true).font_size_offset, 4);
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
