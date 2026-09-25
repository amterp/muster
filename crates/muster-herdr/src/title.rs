//! What a pane's title says once the activity glyph in front of it is gone.
//!
//! herdr already strips one, and announces a title only when the stripped text changes
//! (`observations/herdr-0.8.0.md` section 16). But it strips only from its own list, and a
//! harness that spins a glyph outside that list turns every frame of its spinner into a new
//! title - Claude Code's `◐◑◒◓` did, at several a second per working agent, each one a
//! relabel and a republish of the whole window (section 26). So the rule is repeated here,
//! over herdr's list plus the glyphs harnesses have been seen spinning since.

use serde_json::Value;

/// The title a pane's payload carries, with a leading spinner glyph taken off.
///
/// Read from `terminal_title_stripped`, so herdr's own stripping still applies and this only
/// catches what it missed. `None` when there is no title, or nothing but a glyph.
pub(crate) fn read(pane: &Value) -> Option<String> {
    pane.get("terminal_title_stripped").and_then(Value::as_str).and_then(stripped)
}

fn stripped(title: &str) -> Option<String> {
    let title = title.trim();
    let mut chars = title.chars();
    let rest = match chars.next() {
        Some(first) if is_spinner_glyph(first) => chars.as_str(),
        _ => title,
    };
    // A glyph fused to the next word is part of the title, the same condition herdr applies.
    let title =
        if rest.is_empty() || rest.starts_with(char::is_whitespace) { rest.trim() } else { title };
    (!title.is_empty()).then(|| title.to_string())
}

/// Glyphs seen spinning, not the blocks they come from: `✅` and `❌` sit in Dingbats beside
/// `✳`, and a title that begins with one is using it to say something.
fn is_spinner_glyph(glyph: char) -> bool {
    matches!(
        glyph,
        '·' | '✢' | '✳' | '✶' | '✻' | '✽'   // herdr's own list, Claude Code's older spinner
        | '◐' | '◑' | '◒' | '◓'             // Claude Code's current spinner
        | '\u{2800}'..='\u{28FF}' // Braille, which herdr strips whole: ⠋⠙⠹
    )
}
