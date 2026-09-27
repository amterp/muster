//! What typing into Muster actually sets in motion, with nothing faked.
//!
//! Every piece under this is already judged on its own: the keymap by the conformance corpus,
//! the daemon's key encoding by its own suite, the bridge's drawing by `link.rs`. What none of
//! them can say is whether the pieces are joined. Here a keystroke enters at the seam, leaves
//! the core on the window's input connection, is encoded by a real muster-daemon against the
//! pane's modes, runs a real program, and comes back through a real `muster-bridge` as what a
//! surface would draw.
//!
//! `cat -v` runs in the pane so that what arrived is legible on the screen rather than
//! inferred: an escape sequence renders as `^[[A`.

use crate::support::{Press, Typing};

/// Letters typed at the seam reach the program, and an arrow arrives encoded for the mode the
/// program put the pane in, which only the daemon can see.
#[test]
fn a_keystroke_crosses_the_seam_and_arrives_on_the_panes_screen() {
    let typing = Typing::start("");

    // Application cursor keys go on first, and that is what makes the arrow below worth
    // asserting. In a pane's default mode Up is ESC [ A whoever encodes it, so a test there
    // passes whether or not the pane's modes were consulted. Under DECCKM the correct answer is
    // ESC O A, which the daemon knows from the program's own output and the window never sees.
    typing.run("printf '\\033[?1h'; cat -v");

    // `muster` appears twice: once echoed by the line discipline, which says the bytes reached
    // the PTY, and once written by cat, which says the program read them.
    for key in ["KeyM", "KeyU", "KeyS", "KeyT", "KeyE", "KeyR"] {
        Press::new(key, &key.trim_start_matches("Key").to_lowercase()).send();
    }
    Press::new("Enter", "").send();
    crate::support::until(
        "the typed line to come back from cat",
        || typing.bridge.lines().iter().filter(|line| *line == "muster").count() >= 2,
        || {
            typing
                .bridge
                .diagnosis("the line never arrived, or arrived only as the terminal's echo")
        },
    );

    // `^[OA` is cat -v's rendering of ESC O A, the sequence the daemon chose by reading the
    // pane's modes. `^[[A` here would mean the arrow was encoded for a pane in its default
    // mode, which is the regression this exists to catch.
    Press::new("ArrowUp", "").send();
    typing.expect_on_screen(
        "^[OA",
        "the arrow reached nothing, or reached the pane as ESC [ A rather than the ESC O A \
         this pane's modes call for",
    );
}
