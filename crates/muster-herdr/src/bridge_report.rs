//! What a pane's bridge says about itself, on the socket it already holds.
//!
//! The other direction of `control_stream.rs`, and a different kind of message. What the app
//! sends a bridge is herdr's own JSON, copied through untouched, so the bridge stays a relay
//! with no vocabulary of its own. What comes back is the bridge speaking for itself - one
//! sentence, when it is about to stop - and that is not herdr's to phrase.
//!
//! It exists because the app cannot otherwise learn why a pane went dark. The bridge is the
//! only process that ever sees herdr's closing frame, and it used to write the reason to a log
//! file and exit. The socket closing says a bridge is gone; this says which of the several
//! things that means, and the difference decides whether Muster starts another one.

use muster_core::respawn::Ending;
use serde_json::Value;

/// What a bridge tells the app before it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exiting {
    pub ending: Ending,

    /// herdr's own sentence, when the bridge was given one to pass on.
    ///
    /// Kept whole and untranslated, because it names the terminal and the daemon and Muster
    /// cannot compose either. It is what a person reads when the pane says why it is dark.
    pub reason: Option<String>,

    /// Whether this bridge ever painted anything.
    ///
    /// Separates a pane that ended from a pane that never began, which read identically from
    /// the app: both are a surface showing nothing.
    pub rendered: bool,
}

impl Exiting {
    /// The message as the line that crosses the socket.
    ///
    /// Newline-delimited JSON, matching the direction that already crosses it, so one reader
    /// on either end handles both without a second framing.
    pub fn wire_format(&self) -> Vec<u8> {
        let object = serde_json::json!({
            "type": "bridge.exiting",
            "ending": self.ending.as_str(),
            "reason": self.reason.clone().unwrap_or_default(),
            "rendered": self.rendered,
        });
        let mut out = object.to_string().into_bytes();
        out.push(b'\n');
        out
    }

    /// One line back, or nothing.
    ///
    /// A line this cannot read is skipped rather than fatal, on the same terms as the frame
    /// decoder: the app and the bridge are separate binaries and a mixed pair is an ordinary
    /// state during an upgrade. An unreadable line costs the reason; the socket closing behind
    /// it still reports the exit.
    pub fn parse(line: &[u8]) -> Option<Exiting> {
        let object: Value = serde_json::from_slice(line).ok()?;
        if object.get("type")?.as_str()? != "bridge.exiting" {
            return None;
        }
        let reason = object.get("reason").and_then(Value::as_str).filter(|text| !text.is_empty());
        Some(Exiting {
            ending: object
                .get("ending")
                .and_then(Value::as_str)
                .and_then(Ending::parse)
                .unwrap_or(Ending::Lost),
            reason: reason.map(str::to_string),
            rendered: object.get("rendered").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// What herdr's closing reason means, in Muster's words.
///
/// Matched on herdr's prose, which is the only thing it offers: the closing frame carries a
/// `reason` string and no code (`docs/observations/herdr-0.8.0.md` section 12). Matching prose
/// is fragile, so it is fragile in exactly one place, and an unrecognised reason falls to
/// `Lost` - the ending whose response is to start another bridge, which is the safe way to be
/// wrong. Being wrong the other way would leave a pane dark on a daemon that is perfectly
/// healthy.
pub fn ending(reason: Option<&str>) -> Ending {
    let Some(reason) = reason else { return Ending::Lost };
    if reason.contains("taken over") {
        return Ending::TakenOver;
    }
    if reason.contains("already has an attached client") {
        return Ending::Refused;
    }
    Ending::Lost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_survives_the_round_trip() {
        let sent = Exiting {
            ending: Ending::Refused,
            reason: Some("terminal term_1 already has an attached client".to_string()),
            rendered: false,
        };
        let wire = sent.wire_format();
        assert_eq!(wire.last(), Some(&b'\n'));
        assert_eq!(Exiting::parse(&wire[..wire.len() - 1]), Some(sent));
    }

    #[test]
    fn a_line_from_somewhere_else_is_not_one_of_these() {
        assert_eq!(Exiting::parse(br#"{"type":"terminal.input","bytes":"aA=="}"#), None);
        assert_eq!(Exiting::parse(b"not json at all"), None);
    }
}
