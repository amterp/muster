//! Whether reading the config file again reaches a pane that was already open.
//!
//! This is the whole of what made a reload worth not half-doing. Bindings are easy - the menu
//! is rebuilt from the answer - but a pane's key encoder is built once, when the pane is
//! attached, from the settings in force at that moment. A reload that only replaced the
//! settings would take effect on panes opened afterwards and leave the rest alone, so what
//! `option_as_alt` meant would depend on when each pane happened to be opened: a window that
//! disagrees with itself, invisibly, with no way to tell which pane is right.
//!
//! So the pane here is opened *before* the file changes, and typed into *after*. Both halves of
//! the typing settings are exercised, because they take two different routes into a pane - the
//! keymap answers `[text]` before any encoder is consulted, and `option_as_alt` is the encoder's
//! own flag plus the step that decides whether a keystroke arrives in a shape that reaches it.
//!
//! `cat -v` renders what arrived, so the difference is visible rather than inferred: `^[t` is an
//! escape prefix and a meta chord, where the dagger the layout composed instead arrives as the
//! character itself - valid UTF-8, which `cat -v` passes through untouched.
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use muster::proto::{ReloadConfig, request};
use support::{Press, Typing, answer, assert_ok};

#[test]
fn reading_the_config_again_reaches_a_pane_that_was_already_open() {
    // Started on the defaults: option composes, and no chord stands for bytes.
    let typing = Typing::start("");
    typing.run("cat -v", "cat");

    // The pane exists and is typeable before anything changes, which is the point.
    Press::new("KeyT", "†").modifiers(&["alt"], &["alt"]).without_option("t").send();
    typing.expect_on_screen(
        "\u{2020}",
        "opt+t did not arrive as the composed dagger, so this pane did not start on the \
         defaults and the rest of this test would be proving nothing",
    );

    // The same file, rewritten the way somebody editing it would.
    let path = typing
        .daemon
        .muster_config_with("option_as_alt = \"left\"\n\n[text]\n\"ctrl+g\" = \"muster\"\n");
    assert!(path.exists(), "the harness wrote the config somewhere else on the second call");
    assert_ok(&answer(request::Payload::ReloadConfig(ReloadConfig {})));

    // Now the same chord, into the same pane, with no relaunch and no new surface.
    Press::new("KeyT", "†").modifiers(&["alt"], &["alt"]).without_option("t").send();
    typing.expect_on_screen(
        "^[t",
        "opt+t still arrived as the composed dagger after the config was read again, so the \
         reload replaced the settings and never rebuilt the encoder this pane was attached \
         with - which is the half-done reload this test exists to prevent",
    );

    // And the other route in, which the keymap answers before any encoder sees it. ctrl+g
    // would otherwise be BEL, so what arrives says which of the two layers answered.
    Press::new("KeyG", "").modifiers(&["control"], &[]).send();
    typing.expect_on_screen(
        "muster",
        "the chord bound in [text] by the reloaded file reached the pane as whatever the \
         encoder made of ctrl+g, so the reload did not rebuild this pane's keymap",
    );
}
