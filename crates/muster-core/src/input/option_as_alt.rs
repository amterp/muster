//! Whether the option key acts as alt: the app's setting rather than any pane's.

/// Whether the macOS option key acts as alt rather than composing text.
///
/// Not a pane mode: a local preference, which travels with each keystroke to the daemon.
///
/// Four-valued rather than a flag because the per-side settings are the ones people
/// actually pick: option composes accented characters on macOS, so a common arrangement is
/// right-option-as-alt for meta chords with left option still composing.
///
/// Spelled out as strings because this is configuration a person writes in a file and a
/// value that has to survive the corpus, the log and the shell/core schema. An enum without
/// names travels as an integer nobody can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptionAsAlt {
    #[default]
    Never,
    Always,
    LeftOnly,
    RightOnly,
}

impl OptionAsAlt {
    pub fn parse(name: &str) -> Option<OptionAsAlt> {
        match name {
            "never" => Some(OptionAsAlt::Never),
            "always" => Some(OptionAsAlt::Always),
            "leftOnly" => Some(OptionAsAlt::LeftOnly),
            "rightOnly" => Some(OptionAsAlt::RightOnly),
            _ => None,
        }
    }

    /// The same value, under the name somebody writes in a config file.
    ///
    /// `left` and `right` rather than `leftOnly` and `rightOnly`, on the same grounds the
    /// chord grammar reads `cmd` and `left` instead of `super` and `ArrowLeft`: the wire's
    /// spelling is right for a corpus and wrong for a file that gets hand-edited. Falls
    /// through to [`OptionAsAlt::parse`], so a name copied out of a log line also reads.
    pub fn read(name: &str) -> Option<OptionAsAlt> {
        match name {
            "left" => Some(OptionAsAlt::LeftOnly),
            "right" => Some(OptionAsAlt::RightOnly),
            other => OptionAsAlt::parse(other),
        }
    }

    /// Every value, under the name a config file writes it with.
    ///
    /// The list a refusal quotes back at somebody who misspelled one.
    pub const READABLE: [&'static str; 4] = ["never", "always", "left", "right"];

    pub fn as_str(self) -> &'static str {
        match self {
            OptionAsAlt::Never => "never",
            OptionAsAlt::Always => "always",
            OptionAsAlt::LeftOnly => "leftOnly",
            OptionAsAlt::RightOnly => "rightOnly",
        }
    }
}
