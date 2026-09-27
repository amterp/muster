//! Another bridge takes a pane, and this window leaves it alone.
//!
//! The property `corpus/conformance/respawn.json` calls "the plain first attach": a second
//! Muster window opening onto a pane the first is rendering must not take it away. That used
//! to hold for the dull reason that no bridge death reached the replacement policy at all.
//! Once one does, the same rule that recovers a relaunched window - attach again, with
//! `--takeover` - would answer a takeover by taking it back, and the window on the other side
//! would answer that the same way. One pane, traded at the speed a bridge starts, until both
//! windows ran out of tries.
//!
//! So the ending has to reach the policy and not only the fact. This drives it with a real
//! second bridge rather than a fabricated report, because what separates the two endings is
//! the reason the daemon gives the displaced bridge for letting it go, and the word the bridge
//! turns that into on its link.

use std::process::{Command, Stdio};

use crate::support::{Typing, restarts, until};

/// A bridge displaced by another one says it was taken over, and the window answers that by
/// starting nothing: its `bridge_restarts` for the pane stays at zero.
#[test]
fn a_pane_taken_by_another_bridge_is_left_to_it() {
    let mut typing = Typing::start("");
    let pane = typing.pane.clone();
    assert_eq!(restarts(&pane), Some(0), "a pane nobody has replaced is on none");

    // A second bridge, asking the way a second window's replacement would. It reports to no
    // window of this one's: its own window is the other side of the trade, and dialling this
    // window's link would make it this window's bridge instead. `Typing::start` has already
    // waited for the first bridge to say it attached, so this displaces a bridge holding the
    // pane rather than racing its attach - which would be a refusal, a different ending with a
    // different answer.
    let mut thief = Command::new(env!("CARGO_BIN_EXE_muster-bridge"))
        .arg(&pane)
        .arg("--daemon-socket")
        .arg(typing.daemon.socket_path())
        .arg("--takeover")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("cargo builds muster-bridge before this test runs");

    // The bridge going is what proves the takeover happened; the core logging what it decided
    // is what makes the assertion below a decision rather than a race this test won.
    until("the bridge to be displaced", || typing.bridge.has_exited(), ());
    typing.expect_logged(
        "bridge.yielded",
        "the core never decided anything about a bridge that was taken over, so the count \
         below would be read before the core had acted on the ending",
    );

    assert_eq!(
        restarts(&pane),
        Some(0),
        "this window answered a takeover by taking the pane back, which the window on the \
         other side answers the same way - the pane then belongs to whichever of them runs out \
         of replacements last"
    );

    let _ = thief.kill();
    let _ = thief.wait();
}
