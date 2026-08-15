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
}

impl Default for Presentation {
    /// Open, because the list is how a pane nobody is showing gets found at all - and on a
    /// first launch nobody has decided otherwise.
    fn default() -> Presentation {
        Presentation { sidebar: true }
    }
}

impl Presentation {
    #[must_use]
    pub fn with_sidebar(self, sidebar: bool) -> Presentation {
        Presentation { sidebar }
    }
}
