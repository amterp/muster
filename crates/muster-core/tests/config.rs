//! What a config file means. Cases and their reasoning live in corpus/conformance/config.json.
//!
//! A case's file is a list of lines rather than one string with escapes in it, because a
//! reviewer has to be able to see the TOML they are judging (`docs/testing.md`: cases are
//! text files a reviewer can read).

mod support;

use conformance::Conformance;
use muster_core::config;
use muster_core::input::{Action, Bindings, Chord, Modifiers};
use serde_json::{Value, json};
use support::backend::describe_daemon;

/// The knobs a file set, as lines, leaving out the ones it left alone.
///
/// Exact equality against the default, which is what is wanted: the question is whether the
/// file named this key at all, and a file that wrote the default value did name it. A
/// tolerance here would hide a case asserting a multiplier a hair off the one it meant.
#[allow(clippy::float_cmp)]
fn feel(feel: &config::Feel) -> Vec<String> {
    let mut set = Vec::new();
    if let Some(step) = feel.resize_step {
        set.push(format!("resize_step={step}"));
    }
    if feel.scroll_multiplier != config::Feel::default().scroll_multiplier {
        set.push(format!("scroll_multiplier={}", feel.scroll_multiplier));
    }
    set
}

/// What a file said about how Muster should look, spelled the way the file spells it.
///
/// Only what was named, on the same terms as [`feel`]: absent means the renderer's own, and a
/// case about a font family should not have to state fifteen colours it says nothing about.
/// The values are the parsed ones written back out, so a case pins what Muster understood -
/// `#ABCDEF` read as `#abcdef` is the parser working, and `hollow` surviving as `hollow` is
/// what proves the shell is handed Muster's word rather than a renderer's.
fn appearance(appearance: &config::Appearance) -> Vec<String> {
    let mut set = Vec::new();

    if let Some(family) = &appearance.font.family {
        set.push(format!("font.family={family}"));
    }
    if let Some(size) = appearance.font.size {
        set.push(format!("font.size={size}"));
    }

    let colors = &appearance.colors;
    for (name, color) in [
        ("background", colors.background),
        ("foreground", colors.foreground),
        ("cursor", colors.cursor),
        ("cursor_text", colors.cursor_text),
        ("selection_background", colors.selection_background),
        ("selection_foreground", colors.selection_foreground),
        ("divider", colors.divider),
    ] {
        if let Some(color) = color {
            set.push(format!("colors.{name}={color}"));
        }
    }
    if let Some(palette) = &colors.palette {
        // One line rather than sixteen, because a case about a palette is about the set.
        let entries: Vec<String> = palette.iter().map(ToString::to_string).collect();
        set.push(format!("colors.palette={}", entries.join(" ")));
    }

    if let Some(style) = appearance.cursor.style {
        set.push(format!("cursor.style={}", style.as_str()));
    }
    if let Some(blink) = appearance.cursor.blink {
        set.push(format!("cursor.blink={blink}"));
    }
    if let Some(padding) = appearance.pane_padding {
        set.push(format!("pane_padding={padding}"));
    }

    set
}

/// What a file said a pane should be, on the same terms as [`appearance`].
///
/// Written back out rather than echoed, so a case pins what Muster understood: `non_login`
/// surviving as `non_login` is what proves the daemon is handed Muster's word for it.
fn panes(panes: &config::Panes) -> Vec<String> {
    let mut set = Vec::new();
    if let Some(bytes) = panes.scrollback_bytes {
        set.push(format!("scrollback_bytes={bytes}"));
    }
    if let Some(command) = &panes.shell.command {
        set.push(format!("shell.command={command}"));
    }
    if panes.shell.mode != config::ShellMode::default() {
        set.push(format!("shell.mode={}", panes.shell.mode.as_str()));
    }
    set
}

#[test]
fn config_conformance() {
    let corpus = Conformance::load("config.json");

    let ran = corpus.run(|given| {
        let text = file(given);
        Ok(match config::parse(&text) {
            Ok(parsed) => conformance::fields([
                (
                    "daemons",
                    Some(json!(parsed.daemons.iter().map(describe_daemon).collect::<Vec<_>>())),
                ),
                // What the file changed, rather than every binding in every case. A keymap is
                // partial by design, so what a case is about is the difference.
                ("keymap", Some(json!(rebound(&parsed.bindings)))),
                ("option_as_alt", Some(json!(parsed.input.option_as_alt.as_str()))),
                // The bytes, not the string, because deciding exactly what reaches a pane is
                // the whole of what this setting is for. A case expecting "\n" would pass on
                // a parser that sent the two characters backslash and n.
                (
                    "text",
                    Some(json!(
                        parsed
                            .input
                            .text
                            .iter()
                            .map(|(binding, bytes)| {
                                format!(
                                    "{}={}",
                                    spell(Chord::new(binding.key, binding.modifiers)),
                                    conformance::hex(bytes),
                                )
                            })
                            .collect::<Vec<_>>()
                    )),
                ),
                // Only what the file set, so the two dozen cases about daemons and keymaps do
                // not each carry two knobs and a dozen colours they say nothing about.
                ("feel", Some(json!(feel(&parsed.feel))).filter(|set| set != &json!([]))),
                (
                    "appearance",
                    Some(json!(appearance(&parsed.appearance))).filter(|set| set != &json!([])),
                ),
                ("panes", Some(json!(panes(&parsed.panes))).filter(|set| set != &json!([]))),
            ]),
            // The refusal itself, not a code. Whether the sentence names the key somebody
            // mistyped is the whole of what this file is protecting, and a taxonomy of error
            // kinds would let the wording rot while every case still passed.
            Err(refusal) => json!({ "refused": refusal }),
        })
    });
    assert!(ran > 0, "the config corpus ran no cases, which passes without proving anything");
}

/// Native rather than a case, because half of what it asserts is written by the TOML parser.
///
/// Pinning the parser's own wording in the corpus would make a routine dependency bump look
/// like a behavior change, and eliding it would leave the case asserting nothing. What
/// Muster owns here is that the file is refused whole, that the sentence says what that
/// costs, and that the parser's account of which line went wrong is carried through rather
/// than swallowed.
#[test]
fn malformed_toml_is_refused_with_the_parser_s_account_of_it() {
    let refusal = config::parse("[[daemon]]\nid = \"local").expect_err("this file is not TOML");

    assert!(refusal.contains("not valid TOML"), "{refusal}");
    assert!(refusal.contains("none of it was applied"), "{refusal}");
    assert!(
        refusal.contains("The parser says:") && refusal.len() > "The parser says:".len() + 40,
        "the parser's own message is what names the line, and it did not survive: {refusal}"
    );
}

/// Turning a step in points into the cells the daemon resizes by.
///
/// Native rather than a corpus case: the corpus judges what a config file *means*, and this is
/// arithmetic done later, on a measurement no config file contains. Pinning it here keeps the
/// numbers beside the rule they came from.
#[test]
fn a_step_in_points_becomes_the_cells_that_fit_in_it() {
    let step = config::ResizeStep::Points(150);

    // A cell is roughly 8 points wide and 17 tall, which is the asymmetry two units exist for:
    // one number in points crosses about as much screen either way, where one number in cells
    // does not.
    assert_eq!(step.cells(Some(8.0)), Some(19.0), "150 points is about nineteen columns");
    assert_eq!(step.cells(Some(17.0)), Some(9.0), "the same 150 points is about nine rows");
}

#[test]
fn a_step_in_cells_needs_no_measurement_and_ignores_the_one_it_is_given() {
    let step = config::ResizeStep::Cells(4);

    assert_eq!(step.cells(None), Some(4.0));
    assert_eq!(step.cells(Some(8.0)), Some(4.0), "cells are the identity case of the conversion");
}

#[test]
fn a_step_too_small_to_cross_a_cell_still_moves_one() {
    // Rounding alone would answer zero, which reaches the seam as proto3's unset and silently
    // becomes the daemon's own step - a chord that looks broken rather than small.
    assert_eq!(config::ResizeStep::Points(3).cells(Some(17.0)), Some(1.0));
}

#[test]
fn a_step_in_points_with_nothing_to_divide_by_has_no_answer() {
    // The caller is expected to fall back to the daemon's own step and say so. Inventing a
    // number here would be wrong by whatever the font happens to be.
    assert_eq!(config::ResizeStep::Points(150).cells(None), None);
    assert_eq!(config::ResizeStep::Points(150).cells(Some(0.0)), None, "a cell of no width");
    assert_eq!(config::ResizeStep::Points(150).cells(Some(f32::NAN)), None);
}

/// The bindings this file moved, spelled `action=chord` in bit order.
///
/// Against the defaults rather than in full, because a case listing fifteen unchanged
/// bindings buries the one it is about.
fn rebound(bindings: &Bindings) -> Vec<String> {
    let defaults = Bindings::default();
    let mut changed = Vec::new();
    for action in Action::ALL {
        let now = bindings.chord(action);
        if now == defaults.chord(action) {
            continue;
        }
        changed.push(match now {
            Some(chord) => format!("{}={}", action.as_str(), spell(chord)),
            None => format!("{}=(unbound)", action.as_str()),
        });
    }
    changed
}

fn spell(chord: Chord) -> String {
    let mut spelled: Vec<&str> = Modifiers::ALL_NAMES
        .into_iter()
        .filter(|(_, bit)| Modifiers::CHORD.contains(*bit) && chord.modifiers.contains(*bit))
        .map(|(name, _)| name)
        .collect();
    spelled.push(chord.key.as_str());
    spelled.join("+")
}

fn file(given: &Value) -> String {
    given
        .get("file")
        .and_then(Value::as_array)
        .map(|lines| lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}
