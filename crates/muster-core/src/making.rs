//! What a request to make a pane says when it is not done, in the window and in the CLI with no
//! window alike, so that the two paths refuse the same requests in the same words.

/// Why a pane made hidden cannot also take the keyboard.
pub const HIDDEN_AND_FOCUSED: &str = "a pane cannot be made hidden and take the keyboard: no \
     window shows a hidden pane, so there would be nothing for the keyboard to be on. Nothing was \
     made. Make it hidden, or make it with the keyboard and hide it later.";

/// The name of the hidden tab a maker's helpers share, made with the first of them (MIP-7): the
/// maker's own name, as it was then.
pub fn helpers_tab(maker: &str) -> String {
    format!("{maker}'s helpers")
}

/// The generation Muster names a maker's helpers' tab at, both when it is made and when Show
/// Helpers renames it after its maker. Renaming a tab puts it above every generation it has
/// had, so a helpers' tab still at this one has only ever had the name Muster gave it, and
/// Muster may give it the maker's current one.
pub const HELPERS_TAB_GENERATION: u64 = 1;

/// A daemon was asked to make `pane` and never said what came of it, for `detail`.
///
/// Worded so that nobody reads it as a refusal, because the one thing a caller must not do with
/// it is what a refusal invites: ask again, and make a second pane.
pub fn may_have_made(pane: &str, detail: &str) -> String {
    format!(
        "the daemon was asked to make pane {pane} and never said what came of it ({detail}), so \
         it may well exist and be running what it was asked to run. If it does, `muster window` \
         lists it under that name once the daemon's own event arrives, and `muster pane close \
         --pane {pane}` ends it. Asking again may make a second one."
    )
}
