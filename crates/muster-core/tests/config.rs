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

#[test]
fn config_conformance() {
    let corpus = Conformance::load("config.json");

    let ran = corpus.run(|given| {
        let text = file(given);
        Ok(match config::parse(&text) {
            Ok(parsed) => json!({
                "daemons": parsed.daemons.iter().map(describe_daemon).collect::<Vec<_>>(),
                // What the file changed, rather than all fifteen bindings in every case. A
                // keymap is partial by design, so what a case is about is the difference.
                "keymap": rebound(&parsed.bindings),
                "option_as_alt": parsed.input.option_as_alt.as_str(),
                // The bytes, not the string, because deciding exactly what reaches a pane is
                // the whole of what this setting is for. A case expecting "\n" would pass on
                // a parser that sent the two characters backslash and n.
                "text": parsed
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
                    .collect::<Vec<_>>(),
            }),
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
