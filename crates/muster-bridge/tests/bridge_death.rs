//! A real bridge dies, and the window gets another one.
//!
//! The gap kan a_2IRcMjFs0 is about. `corpus/conformance/respawn.json` drives the replacement
//! policy directly and `crates/muster-seam/tests/respawn.rs` drives the seam request that
//! reaches it, so both halves were covered and the wiring between them was covered nowhere -
//! and the wiring was the part that did not exist. Two field runs on 0.4.1 killed nine bridges
//! between them and produced not one replacement.
//!
//! So this one uses a real bridge process and ends it the way a machine going away would. What
//! it asserts is the number the shell reads: a `bridge_restarts` that moved is what makes a
//! window build a new surface, and building one is the only way a bridge is ever started.
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use support::{Typing, restarts, until};

#[test]
fn a_bridge_that_dies_is_noticed_and_replaced() {
    let mut typing = Typing::start("");
    let pane = typing.pane.clone();
    assert_eq!(restarts(&pane), Some(0), "a pane nobody has replaced is on none");

    typing.bridge.kill();

    until(
        "the core to notice the bridge is gone and ask for another",
        || restarts(&pane) == Some(1),
        || {
            format!(
                "  Impact: nothing noticed this pane's bridge die, so no replacement was asked \
                 for and the pane stays dark until the app is relaunched - which is the whole \
                 of kan a_2IRcMjFs0.\n  The last thing the core published about {pane}: \
                 {:?}\n  Look for `channel.bridge.gone` and `bridge.ended` in the run log; \
                 neither appearing means the exit never reached the core at all.",
                restarts(&pane)
            )
        },
    );
}
