//! What a pane's bridge says about itself, on the socket it already holds.
//!
//! The other direction of `control_stream.rs`, and a different kind of message. What the app
//! sends a bridge is herdr's own JSON, copied through untouched, so the bridge stays a relay
//! with no vocabulary of its own. What comes back is the bridge speaking for itself, in Muster's
//! words, about the two things only it knows.
//!
//! **Why the pane went dark.** The bridge is the only process that ever sees herdr's closing
//! frame, and it used to write the reason to a log file and exit. The socket closing says a
//! bridge is gone; `Exiting` says which of the several things that means, and the difference
//! decides whether Muster starts another one.
//!
//! **How big a grid it asked for.** The shell measures a region in points and the daemon is only
//! ever told, so the number that decides whether a frame of this pane fits under herdr's cap
//! exists nowhere but here. `Grid` carries it, on every resize rather than only when it looks
//! bad - what counts as bad is Muster's rule and belongs above this seam, and a bridge that only
//! reported trouble could never say the trouble was over.

use muster_core::respawn::Ending;
use serde_json::Value;

/// How big a grid a bridge has just asked its daemon to draw.
///
/// The second thing a bridge speaks for itself about, and it exists because nothing else in the
/// window knows this number. The bridge reads its PTY and passes `--cols` and `--rows` to herdr,
/// so this is the grid the daemon actually renders - not the shell's arithmetic about a region,
/// and not something a daemon reports back. Past about a hundred thousand cells a frame of it
/// exceeds herdr's 2 MiB cap and is skipped, which stops the pane updating while everything
/// downstream reports health (kan a_2KHGYMpnK).
///
/// Sent on every resize rather than only when it looks bad, because what counts as bad is
/// Muster's rule and belongs above this seam - and a bridge that only reported trouble could
/// never say the trouble was over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub columns: u32,
    pub rows: u32,
}

impl Grid {
    /// The message as the line that crosses the socket, framed like the one beside it.
    pub fn wire_format(&self) -> Vec<u8> {
        let object = serde_json::json!({
            "type": "bridge.sized",
            "cols": self.columns,
            "rows": self.rows,
        });
        let mut out = object.to_string().into_bytes();
        out.push(b'\n');
        out
    }

    /// One line back, or nothing.
    ///
    /// A line this cannot read is skipped rather than fatal, on the same terms as `Exiting`: the
    /// app and the bridge are separate binaries and a mixed pair is an ordinary state during an
    /// upgrade. An older bridge sends none of these at all, and a window reading none of them
    /// behaves exactly as it did before they existed - no ceiling, and no accusation either.
    pub fn parse(line: &[u8]) -> Option<Grid> {
        let object: Value = serde_json::from_slice(line).ok()?;
        if object.get("type")?.as_str()? != "bridge.sized" {
            return None;
        }
        Some(Grid {
            columns: u32::try_from(object.get("cols")?.as_u64()?).ok()?,
            rows: u32::try_from(object.get("rows")?.as_u64()?).ok()?,
        })
    }
}

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

    #[test]
    fn a_grid_survives_the_round_trip() {
        let sent = Grid { columns: 931, rows: 248 };
        let wire = sent.wire_format();
        assert_eq!(wire.last(), Some(&b'\n'));
        assert_eq!(Grid::parse(&wire[..wire.len() - 1]), Some(sent));
    }

    /// The two messages share one socket and one reader, so each has to refuse the other's
    /// lines rather than half-reading them - a grid taken for an exit would end a pane that
    /// was only resized.
    #[test]
    fn the_two_messages_do_not_answer_for_each_other() {
        let grid = Grid { columns: 80, rows: 24 }.wire_format();
        assert_eq!(Exiting::parse(&grid[..grid.len() - 1]), None);
        let exiting = Exiting { ending: Ending::Lost, reason: None, rendered: true }.wire_format();
        assert_eq!(Grid::parse(&exiting[..exiting.len() - 1]), None);
    }
}
