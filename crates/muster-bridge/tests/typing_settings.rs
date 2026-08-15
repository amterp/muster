//! Whether a config file's answers about typing reach the pane, or stop at the parser.
//!
//! The corpus already says what `option_as_alt` and `[text]` mean, and what the encoder
//! produces under each. What it cannot say is whether the file is read, carried across the
//! seam and handed to the encoder a pane was attached with - which is exactly where this
//! sat before: `TerminalModeProfile` has always had the setting and nothing set it, so every
//! conformance case passed while every window ran on the default.
//!
//! So this asserts through the file. `cat -v` renders what arrived, so the difference
//! between the two answers is visible rather than inferred: `^[t` is an escape prefix and a
//! meta chord, where the dagger option composed instead would arrive as its three UTF-8
//! bytes and render as `M-bM-^@M- `.
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use support::{Press, Typing};

#[test]
fn the_config_files_answers_about_typing_reach_the_pane() {
    let typing = Typing::start("option_as_alt = \"left\"\n\n[text]\n\"ctrl+g\" = \"muster\"\n");
    typing.run("cat -v", "cat");

    // opt+t, as macOS reports it: option is held, and the layout already spent it turning
    // the key into U+2020. Believing that spend is what the default does, and it is why an
    // agent bound to alt+t hears nothing.
    Press::new("KeyT", "†").modifiers(&["alt"], &["alt"]).without_option("t").send();
    typing.expect_on_screen(
        "^[t",
        "opt+t reached the pane as the composed dagger rather than as a meta chord, so the \
         config file's `option_as_alt` was parsed and then never reached the encoder this \
         pane was attached with",
    );

    // A chord standing for bytes, which no encoder is consulted about. ctrl+g would
    // otherwise be BEL, so what arrives says which of the two layers answered.
    Press::new("KeyG", "").modifiers(&["control"], &[]).send();
    typing.expect_on_screen(
        "muster",
        "the chord bound in [text] reached the pane as whatever the encoder made of ctrl+g \
         rather than as the bytes the file named",
    );
}
