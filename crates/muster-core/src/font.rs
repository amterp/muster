//! What to say when the font somebody named is not the font they will get.
//!
//! `[font] family` is the one appearance value the core cannot check for itself. Which fonts a
//! machine has is a platform question, and Muster's whole answer to a value it cannot act on is
//! to hand it to the renderer and let the renderer decide - which for a family name means
//! accepting any string at all, falling back, and painting. So `family = "Fira Cod"` looks
//! exactly like `family = "Fira Code"`, on a machine with either or neither.
//!
//! The split is the one the window frame and the locale already draw: the shell reports what
//! only it can observe, and the decision lives here, where a case can reach it. What the shell
//! reports is deliberately about the one family rather than a list of every family installed -
//! enumerating them costs a tenth of a second, and nothing here needs to know what else is
//! there.
//!
//! A warning rather than a refusal, which is the one decision in this file that could have gone
//! the other way. A colour that will not parse is wrong on every machine, so refusing the file
//! is the same answer everywhere; a font is wrong only here, and somebody keeping one config on
//! a laptop and a devenv would find their file refused on one of them. Refusing is also out of
//! proportion: Muster refuses a config file whole, so a misspelled font would take the keymap
//! down with it.

use crate::problems::{Problem, Severity};

/// What the platform said about the family the config named.
///
/// `monospaced` means nothing when `found` is false - there is no font to have the trait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontReport {
    /// The family `[font] family` asked for, empty when it named none.
    pub family: String,
    pub found: bool,
    pub monospaced: bool,
}

/// The condition this report describes, or nothing when the window will paint what was asked.
///
/// One key for both ways of being wrong, because they are one condition - the family you named
/// is not what will paint - and they cannot both be true of one family. Two keys would let a
/// window claim both at once.
pub const KEY: &str = "font.family";

#[must_use]
pub fn problem(report: &FontReport) -> Option<Problem> {
    // Naming nothing is the design rather than an oversight: absent means the renderer's own
    // default, because Muster has no opinion about which monospace font a machine has.
    if report.family.is_empty() {
        return None;
    }

    if !report.found {
        return Some(Problem {
            key: KEY.to_string(),
            severity: Severity::Warning,
            // What it fell back to is stated as the renderer's own default rather than named,
            // and that is the honest limit: the renderer accepts a family name as a string and
            // offers no way to ask which font it ended up with, so any name here would be a
            // guess presented as a fact.
            detail: format!(
                "`{}` in the config file's [font] family is not a font family this machine \
                 has, so panes are painted with the renderer's own default instead. Check the \
                 spelling, and whether the font is installed for this user rather than for \
                 another one.",
                report.family
            ),
        });
    }

    if !report.monospaced {
        return Some(Problem {
            key: KEY.to_string(),
            severity: Severity::Warning,
            detail: format!(
                "`{}` in the config file's [font] family is installed but is not monospaced, so \
                 columns will not line up and the grid will look wrong. That is the font rather \
                 than Muster - a terminal needs every cell the same width. Name a monospace \
                 family, or leave `family` out for the renderer's own.",
                report.family
            ),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{FontReport, problem};

    /// The trait is not consulted for a family nobody has, because there is no font to have it.
    /// A shell reporting `monospaced: false` alongside `found: false` is reporting the absence
    /// of a font rather than a proportional one, and answering that with the wrong complaint
    /// would send somebody looking at a font they do not have.
    #[test]
    fn a_family_nobody_has_is_reported_as_missing_and_not_as_proportional() {
        let raised = problem(&FontReport {
            family: "Fira Cod".to_string(),
            found: false,
            monospaced: false,
        })
        .expect("a family nobody has is worth saying");
        assert!(raised.detail.contains("Fira Cod"));
        assert!(!raised.detail.contains("monospaced"));
    }
}
