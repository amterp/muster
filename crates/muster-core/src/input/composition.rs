//! Decides who a keystroke belonged to: the input method, or the pane.
//!
//! macOS routes every key press through the input method first, and the method answers in
//! two ways at once - it may commit text, and it may leave a composition in progress. The
//! host has to read those two signals and pick exactly one thing to send. Picking *both* is
//! the failure mode: committed text and an encoded keystroke are two renderings of the same
//! press, and sending both makes `hello` arrive as `hheelllloo`.
//!
//! A value rather than a branch inside `keyDown`, because "exactly one" is the whole
//! property and it is worth stating somewhere a test can reach. The AppKit half stays a
//! matter of collecting the three inputs.

/// The one thing to do about a keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A composition finished. Send what it produced, not the key that finished it.
    SendText(String),
    /// An ordinary press. Encode it.
    SendKey,
    /// The input method is still working; the pane has no business seeing this yet.
    SendNothing,
}

/// - `was_composing`: whether a composition was already in progress before this press.
/// - `committed`: text the input method handed over during this press, if any.
/// - `still_composing`: whether a composition remains in progress afterwards.
pub fn outcome(was_composing: bool, committed: Option<&str>, still_composing: bool) -> Outcome {
    // Still composing: the press selected a candidate or extended a preedit.
    if still_composing {
        return Outcome::SendNothing;
    }

    // A composition that just ended is the only case where the committed text is the truth
    // and the keystroke is not - what an input method produces need not resemble the key
    // that produced it. Outside that, AppKit is simply handing back the character that was
    // typed, and the encoder is about to produce the same thing from the key itself.
    match committed {
        Some(text) if was_composing && !text.is_empty() => Outcome::SendText(text.to_string()),
        _ => Outcome::SendKey,
    }
}
