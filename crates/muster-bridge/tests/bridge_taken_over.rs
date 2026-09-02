//! Another client takes a pane's terminal, and this window leaves it alone.
//!
//! The property `corpus/conformance/respawn.json` calls "the plain first attach": a second
//! Muster window opening onto a pane the first is rendering must not take it away. That used
//! to hold for the dull reason that no bridge death reached the replacement policy at all.
//! Once one does, the same rule that recovers a relaunched window - attach again, with
//! `--takeover` - would answer a takeover by taking it back, and the window on the other side
//! would answer that the same way. One terminal, traded at the speed a bridge starts, until
//! both windows ran out of tries.
//!
//! So the ending has to reach the policy and not only the fact. This drives it with a real
//! second client rather than a fabricated report, because what separates the two endings is
//! herdr's own wording and nothing else (`docs/observations/herdr-0.8.0.md` section 23).
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use std::process::{Command, Stdio};

use support::{Typing, restarts, until};

#[test]
fn a_terminal_taken_by_another_client_is_left_to_it() {
    let mut typing = Typing::start("");
    let pane = typing.pane.clone();
    assert_eq!(restarts(&pane), Some(0), "a pane nobody has replaced is on none");

    // Painting first, and waited for rather than assumed. A bridge becomes typeable when it
    // dials back, which is before its own attach has finished - so a second client racing that
    // gap is refused *this* bridge's attach rather than taking a terminal it holds, which is a
    // different ending with a different answer and would make this test flake.
    typing.expect_on_screen(
        "$",
        "the pane never painted a prompt, so this bridge had not attached and a second client \
         would be racing its attach rather than displacing it",
    );

    // A second client, asking the way a second window's replacement bridge would.
    let mut thief = Command::new(herdr_harness::binary())
        .args(["terminal", "session", "control", &pane, "--takeover"])
        .args(["--cols", "80", "--rows", "24"])
        .env("HERDR_SOCKET_PATH", typing.daemon.socket_path())
        // Held open, because a client whose stdin reaches EOF releases the terminal again -
        // which would hand it straight back and prove nothing.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the pinned herdr runs");

    // The bridge going is what proves the takeover happened, and it is also what makes the
    // assertion below a decision rather than a race this test won: the core has been told.
    until("the bridge to be displaced", || typing.bridge.has_exited(), ());
    until("the core to have acted on it", || restarts(&pane).is_some(), ());

    assert_eq!(
        restarts(&pane),
        Some(0),
        "this window answered a takeover by taking the terminal back, which the window on the \
         other side answers the same way - the pane then belongs to whichever of them runs out \
         of replacements last"
    );

    let _ = thief.kill();
    let _ = thief.wait();
}
