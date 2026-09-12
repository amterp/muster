//! What a pane's bridge says about itself, on the socket it already holds.
//!
//! The other direction of `control_stream.rs`, and a different kind of message. What the app
//! sends a bridge is herdr's own JSON, copied through untouched, so the bridge stays a relay
//! with no vocabulary of its own. What comes back is the bridge speaking for itself, in Muster's
//! words, about the three things only it knows.
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
//!
//! **That it is painting at all.** Frames run from this process's stdout into a surface and never
//! pass the app, so nothing above knows whether a pane answered what was typed into it - and a
//! pane that stopped answering looks exactly like one nobody has touched. `Painted` says a frame
//! arrived, on the same terms as `Grid`: the fact, not a verdict about it.

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

/// That a bridge has painted, and how much of it since it last said so.
///
/// The third thing a bridge speaks for itself about, and the one nothing above it can observe.
/// Frames go from this process's stdout into a surface's command, so the app never sees one - and
/// a pane that has stopped painting is indistinguishable from a pane whose agent has nothing to
/// say. Joined above this seam with the one fact the app does have, which is what it delivered,
/// that difference becomes a pane that was asked for something and answered nothing
/// (kan a_2LMRCug0P).
///
/// Sent at most every [`PAINTED_INTERVAL_NS`] and only when a frame arrived, which is the whole
/// of what makes it affordable: a quiet pane costs nothing, a pane painting flat out costs four
/// small lines a second, and either way the app learns "it painted" rather than a per-frame
/// stream it would have to summarize itself.
///
/// The counts are carried because they cost nothing and make the line worth reading in a log -
/// a bridge sending frames of zero bytes is a different bug from one sending none. Nothing above
/// reads them to decide anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Painted {
    pub frames: u64,
    pub bytes: u64,
}

/// How often a bridge says it is painting, in nanoseconds.
///
/// Its own interval rather than the one the log summary runs on, because the two have different
/// readers. A second is right for a person reading repaint counts, and a quarter of one is what
/// bounds the window in which a keystroke can land inside a burst of painting and be recorded as
/// unanswered - the last frames of a burst are only reported by the next line, and there is no
/// next line when the burst was the end of it.
pub const PAINTED_INTERVAL_NS: u64 = 250_000_000;

impl Painted {
    /// The message as the line that crosses the socket, framed like the two beside it.
    pub fn wire_format(&self) -> Vec<u8> {
        let object = serde_json::json!({
            "type": "bridge.painted",
            "frames": self.frames,
            "bytes": self.bytes,
        });
        let mut out = object.to_string().into_bytes();
        out.push(b'\n');
        out
    }

    /// One line back, or nothing.
    ///
    /// A line this cannot read is skipped rather than fatal, on the same terms as the two beside
    /// it: the app and the bridge are separate binaries and a mixed pair is an ordinary state
    /// during an upgrade. An older bridge sends none of these, and a window reading none of them
    /// behaves exactly as it did before they existed - no accusation, and no false reassurance
    /// either, because the watch above is driven by what a bridge says rather than by its silence.
    pub fn parse(line: &[u8]) -> Option<Painted> {
        let object: Value = serde_json::from_slice(line).ok()?;
        if object.get("type")?.as_str()? != "bridge.painted" {
            return None;
        }
        Some(Painted {
            frames: object.get("frames")?.as_u64()?,
            bytes: object.get("bytes")?.as_u64()?,
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
///
/// That is also why each of the others is matched on words recorded from a real daemon, and why
/// `crates/muster-herdr/tests/one_client_per_terminal.rs` records the one for a closed pane again
/// on every run: a `Gone` read into the wrong prose starts no bridge for a pane that needed one.
pub fn ending(reason: Option<&str>) -> Ending {
    let Some(reason) = reason else { return Ending::Lost };
    if reason.contains("taken over") {
        return Ending::TakenOver;
    }
    if reason.contains("already has an attached client") {
        return Ending::Refused;
    }
    // `terminal attach ended: terminal <id> not found`, which is what a client hears when its pane
    // is closed under it (`corpus/herdr-0.8.0/closing-reasons/closed.jsonl`).
    if reason.contains("not found") {
        return Ending::Gone;
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

    #[test]
    fn a_paint_survives_the_round_trip() {
        let sent = Painted { frames: 12, bytes: 48_120 };
        let wire = sent.wire_format();
        assert_eq!(wire.last(), Some(&b'\n'));
        assert_eq!(Painted::parse(&wire[..wire.len() - 1]), Some(sent));
    }

    /// The three messages share one socket and one reader, so each has to refuse the others'
    /// lines rather than half-reading them - a grid taken for an exit would end a pane that
    /// was only resized, and a paint taken for either would do it four times a second.
    #[test]
    fn the_messages_do_not_answer_for_each_other() {
        let grid = Grid { columns: 80, rows: 24 }.wire_format();
        let painted = Painted { frames: 1, bytes: 2 }.wire_format();
        let exiting = Exiting { ending: Ending::Lost, reason: None, rendered: true }.wire_format();
        for line in [&grid, &painted, &exiting] {
            let line = &line[..line.len() - 1];
            let taken = [
                Grid::parse(line).is_some(),
                Painted::parse(line).is_some(),
                Exiting::parse(line).is_some(),
            ];
            assert_eq!(
                taken.iter().filter(|read| **read).count(),
                1,
                "exactly one reader should take {}, and {taken:?} did",
                String::from_utf8_lossy(line),
            );
        }
    }
}
