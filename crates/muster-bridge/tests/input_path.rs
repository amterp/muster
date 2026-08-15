//! What typing into Muster actually sets in motion, with nothing faked.
//!
//! Every piece under this is already judged on its own: the keymap and the encoder by the
//! conformance corpus, the frame decoder by recorded bytes, the daemon connection by
//! `muster-herdr`'s suite. What none of them can say is whether the pieces are joined. Here
//! a keystroke enters at the seam, is encoded, crosses a socket into a real `muster-bridge`,
//! reaches a real herdr, runs a real program, and comes back as frames.
//!
//! Both routes out of the core are exercised, because they fail differently. Printable keys
//! are encoded locally and leave over the bridge's socket; arrows are handed to the daemon
//! to encode against the pane's real modes and never touch the bridge (`architecture.md`,
//! control plane). `cat -v` runs in the pane so that what arrived is legible on the screen
//! rather than inferred: an escape sequence renders as `^[[A`.
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use support::{Press, Typing};

#[test]
fn a_keystroke_crosses_the_seam_and_arrives_on_the_panes_screen() {
    let typing = Typing::start("");

    // Application cursor keys go on first, and that is what makes the arrow below worth
    // asserting. In a pane's default mode both routes encode Up as ESC [ A, so a test there
    // passes whether or not the daemon did the encoding. Under DECCKM the correct answer is
    // ESC O A and Muster's blind profile still says ESC [ A
    // (`muster_core::input::TerminalModeProfile::UNKNOWN_PANE`), so the two are finally
    // distinguishable - which is the whole reason this key leaves the core over a different
    // channel.
    typing.run("printf '\\033[?1h'; cat -v", "cat");

    // The local route. `muster` appears twice: once echoed by the line discipline, which
    // says the bytes reached the PTY, and once written by cat, which says the program read
    // them.
    for key in ["KeyM", "KeyU", "KeyS", "KeyT", "KeyE", "KeyR"] {
        Press::new(key, &key.trim_start_matches("Key").to_lowercase()).send();
    }
    Press::new("Enter", "").send();
    support::until(
        "the typed line to come back from cat",
        || typing.bridge.lines().iter().filter(|line| *line == "muster").count() >= 2,
        || {
            typing
                .bridge
                .diagnosis("the line never arrived, or arrived only as the terminal's echo")
        },
    );

    // The server-encoded route, which leaves the core for the daemon directly and skips the
    // bridge. `^[OA` is cat -v's rendering of ESC O A, the sequence herdr chose by reading
    // modes Muster cannot see. `^[[A` here would mean the arrow fell back to the local
    // guess, which is the regression this exists to catch.
    Press::new("ArrowUp", "").send();
    typing.expect_on_screen(
        "^[OA",
        "the arrow reached nothing, or reached the pane as the locally guessed ESC [ A \
         rather than the ESC O A this pane's modes call for",
    );
}
