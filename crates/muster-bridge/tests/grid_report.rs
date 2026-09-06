//! A real bridge says how big a grid it asked for, and the window acts on it.
//!
//! The gap kan a_2KHGYMpnK is about, from the end that nothing else can reach. A daemon draws a
//! pane by sending the whole screen as one frame and herdr skips any client frame over 2 MiB, so
//! a pane past about a hundred thousand cells stops updating - and everything downstream reports
//! health while it does. `muster window` said `idle`, the bridge went on relaying keystrokes,
//! the client stayed connected, and the only evidence was a WARN in a log file on the far
//! machine. Sixteen minutes of that.
//!
//! `corpus/conformance/pane-grid.json` drives the rule and the unit tests in `bridge_report`
//! drive the line's spelling, so both ends were coverable and the wire between them was not. It
//! is also the part most likely to break silently: a bridge whose report nothing reads behaves
//! exactly like the window that had no ceiling at all.
//!
//! So this uses a real bridge and asks the window what it thinks is wrong. The ceiling is set
//! below what a bridge on a pipe can produce, because a test cannot make a pane of a hundred
//! thousand cells: the bridge falls back to 80 by 24 when its stdout is not a surface's PTY.
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use herdr_harness::until;
use serde_json::json;
use support::{Typing, named_pane, problems};

/// Below 80 by 24, which is what a bridge whose stdout is a pipe reports. The number is not the
/// subject here - `pane-grid.json` argues about the real one - what is being proved is that a
/// grid crosses the wire at all and reaches the rule.
const CEILING: &str = "100";

#[test]
fn a_pane_too_big_to_draw_is_reported() {
    // SAFETY: nothing else in this process reads the environment concurrently. This runs
    // before the core is started, which is when it reads this.
    unsafe { std::env::set_var("MUSTER_FRAME_CELLS", CEILING) };

    let typing = Typing::start("");
    let pane = typing.pane.clone();

    until(
        "the window to say the pane is too big to draw",
        || problems().iter().any(|key| key.ends_with(":grid")),
        || {
            format!(
                "  Impact: a pane past what one frame can carry stops updating while its state, \
                 its bridge and its client all report health - so nothing in the window says \
                 anything and the only record is a warning in the daemon's own log on its own \
                 machine.\n  What the window says is wrong: {:?}\n  Look for `bridge.resize` in \
                 the run log, which is the bridge deciding the grid, and `pane.grid.oversized`, \
                 which is the window acting on it. The first without the second means the \
                 report never crossed the bridge's control socket.",
                problems()
            )
        },
    );

    // Named rather than matched loosely, because a problem key is what the roster draws a row
    // from and a test that accepted any key at all would pass on somebody else's failure.
    let key = format!("pane:local/{}:grid", named_pane(&pane));
    assert!(
        problems().contains(&key),
        "the window should report this pane by name, and says {:?}",
        problems()
    );

    // And it goes when the pane does. A problem that outlived its pane would be a row naming
    // something nobody can look at - and the pane most likely to be closed is the one that had
    // stopped updating, so this is the ordinary end of the story rather than an edge of it.
    typing.daemon.call("pane.close", &json!({ "pane_id": pane }));
    until(
        "the window to take back what it said about a pane that has gone",
        || !problems().contains(&key),
        || format!("the window still says: {:?}", problems()),
    );
}
