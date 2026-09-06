//! How big a pane's grid is, and what happens when it is bigger than a frame can carry.
//!
//! A daemon paints a pane by sending the whole screen as one frame, and herdr refuses any
//! client frame over 2 MiB - it logs one line on its own machine and skips it. So a pane past
//! roughly a hundred thousand cells stops updating while everything downstream reports perfect
//! health: the client stays connected, the bridge goes on relaying keystrokes, and the agent
//! goes on working. Measured at sixteen minutes of a frozen pane whose only evidence was a WARN
//! in a log file on another machine (kan a_2KHGYMpnK).
//!
//! Muster asked for that grid, so both halves are Muster's. **Do not ask for one a frame cannot
//! carry**, which is what [`Grids::may_shrink`] answers for the chord that walks a pane there.
//! And **say so when it happens anyway**, because a person can still make a window enormous or
//! zoom a pane that was already small.
//!
//! herdr's cap is a sensible guard against an oversized length prefix and is not the thing to
//! argue with. What is worth changing upstream is that a text-only frame over the cap is skipped
//! where an oversized *graphics* frame is degraded - but that is herdr's to decide, and a window
//! that quietly asks for the impossible would still be Muster's bug.
//!
//! Pure - no clock, no sockets. A grid arrives as two numbers, so every rule here is driven by
//! a recorded case.

use std::collections::BTreeMap;

use crate::composition::PaneKey;

/// The most cells a pane may hold before one frame of it stops fitting.
///
/// The number this ships with. It is passed in rather than read here, on the same terms as the
/// deadline the typeable watch runs on: a rule driven by a case has to be answerable at a value
/// the case names, and the suite needs a window whose ceiling a real 80x24 bridge can cross.
///
/// herdr's cap is 2 MiB (`src/protocol/wire.rs`, `MAX_FRAME_SIZE`), and a Claude Code screen
/// was measured at about twenty bytes per cell - so 2,097,152 / 20 is 104,857 cells, rounded
/// down here for content denser than the one that was measured.
///
/// **At the real cap rather than short of it**, which is the whole of choosing this number. A
/// 3440x1440 display fills a single pane with 63,030 cells at 10pt and 78,988 at 9pt with
/// nothing pressed at all, so a ceiling below about eighty thousand would refuse a setup
/// somebody plainly has and accuse a pane that was working - and a false alarm that also
/// disables a working control is worse than the silence this exists to end. At a hundred
/// thousand that display stays silent through 9pt and stops at about 8pt, which is where a
/// frame genuinely stops fitting.
///
/// The cost is a press of overshoot: the one that takes a pane from just under to just over is
/// allowed, because nothing here can predict the next grid - Muster does not know the font size
/// it is offsetting from, which lives on the far side of the renderer seam. Noticing is what
/// catches that press, and noticing is the better detector anyway.
pub const FRAME_CELLS: u32 = 100_000;

/// What the problem list should be told about the panes that have been sized.
///
/// A diff, on the same terms as `typeable::Reported` and for the same reason: the caller's job
/// is to raise and clear, and a pane being resized twice while it is too big is not two pieces
/// of news.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reported {
    /// Panes that have just become too big: the problem key and the whole sentence to say.
    pub raise: Vec<(String, String)>,

    /// Keys that were raised and are no longer true.
    pub clear: Vec<String>,
}

/// How big every pane is, as the bridge that asked the daemon for it last reported.
///
/// The bridge rather than the shell, because the bridge is what actually sets the grid: it
/// reads its PTY and passes `--cols` and `--rows` to herdr, so what it reports is the number
/// that decides whether a frame fits rather than a prediction of it. It also covers every way a
/// pane grows at once - a font chord, a zoom, a window dragged wider - where a measurement sent
/// with one request would cover only that request.
#[derive(Debug, Default)]
pub struct Grids {
    grids: BTreeMap<PaneKey, u32>,

    /// Which panes have already been reported, so that clearing knows what to take back.
    reported: BTreeMap<PaneKey, u32>,
}

impl Grids {
    pub const fn new() -> Grids {
        Grids { grids: BTreeMap::new(), reported: BTreeMap::new() }
    }

    /// A bridge has told the daemon how big to draw this pane.
    ///
    /// Answers what the problem list owes, which is nothing at all for the overwhelmingly
    /// common case of a pane that fits and always did.
    pub fn sized(&mut self, pane: &PaneKey, columns: u32, rows: u32, ceiling: u32) -> Reported {
        let cells = columns.saturating_mul(rows);
        self.grids.insert(pane.clone(), cells);
        if cells >= ceiling {
            // Re-raised on a grid that changed, because the sentence names the grid and a pane
            // being walked further past the ceiling is worth saying again. Not on a repeat of
            // the same numbers: a window republishes for its own reasons, and a problem that
            // reappeared every time would be the nagging keying it by its condition prevents.
            if self.reported.insert(pane.clone(), cells) == Some(cells) {
                return Reported::default();
            }
            return Reported {
                raise: vec![(key(pane), too_big(pane, columns, rows, ceiling))],
                clear: Vec::new(),
            };
        }
        Reported {
            raise: Vec::new(),
            clear: self.reported.remove(pane).map(|_| vec![key(pane)]).unwrap_or_default(),
        }
    }

    /// Whether the text in this pane may get any smaller.
    ///
    /// `false` for a pane already at or over the ceiling, which makes the chord saturate rather
    /// than refuse - text that stops shrinking, the same answer `FontSizes` already gives at the
    /// end of its own range and for the same reason: somebody holding the key down is asking to
    /// keep going, and a refusal for a keystroke whose result they cannot see says nothing.
    ///
    /// A pane nothing has reported a grid for may shrink. That is a pane whose bridge has not
    /// started yet, and refusing a chord on the strength of knowing nothing would disable the
    /// control at exactly the moment a window opens.
    ///
    /// Only shrinking is bounded. Growing and resetting always work, including from over the
    /// ceiling, because they are the way back.
    pub fn may_shrink(&self, pane: &PaneKey, ceiling: u32) -> bool {
        self.grids.get(pane).is_none_or(|cells| *cells < ceiling)
    }

    /// The pane has gone, so its grid and anything said about it mean nothing.
    pub fn forget(&mut self, pane: &PaneKey) -> Reported {
        self.grids.remove(pane);
        Reported {
            raise: Vec::new(),
            clear: self.reported.remove(pane).map(|_| vec![key(pane)]).unwrap_or_default(),
        }
    }

    /// Keeps only the panes named, which is how a window that dropped a daemon lets go.
    pub fn retain(&mut self, keep: impl Fn(&PaneKey) -> bool) {
        self.grids.retain(|pane, _| keep(pane));
        self.reported.retain(|pane, _| keep(pane));
    }
}

/// Names the condition: this one pane is too big to be drawn.
///
/// One key per pane, on the same terms as a pane that cannot be typed into: fourteen panes
/// working and one frozen is the case worth reporting precisely.
///
/// Public because two other things spell it the same way - the corpus, which names the keys it
/// expects, and the seam's own test, which reads them back off the wire.
pub fn key(pane: &PaneKey) -> String {
    format!("pane:{pane}:grid")
}

/// What to tell somebody whose pane has stopped updating, and what to do about it.
///
/// Three things, because a warning that only says what happened leaves the reader starting
/// cold. The grid is named because it is the fact nothing else in the window shows and the one
/// that makes the sentence checkable. The remedy is named in the person's own terms - make the
/// text bigger, or the pane smaller - rather than in cells, because a cell count is not
/// something anybody can act on directly.
///
/// What it must not do is blame the daemon. herdr is behaving exactly as documented, and the
/// window asked for a grid whose frames cannot fit through a cap that exists for a good reason.
pub fn too_big(pane: &PaneKey, columns: u32, rows: u32, ceiling: u32) -> String {
    let cells = u64::from(columns) * u64::from(rows);
    format!(
        "The pane {pane} is {columns} by {rows}, which is {cells} cells - past the {ceiling} \
         a single frame of it can carry. The daemon is drawing this pane and then throwing every \
         frame away, so it has stopped updating on screen while its agent goes on working and \
         every other pane in the window is unaffected. Nothing downstream reports this: the \
         pane's state, its bridge and its client all look healthy, and the only other record is \
         a warning in the daemon's own log on its own machine. Make the text bigger \
         (`muster font larger`, or the font-size chord) or the pane smaller - unzooming it, or \
         a smaller window - and it starts painting again on the next frame."
    )
}
