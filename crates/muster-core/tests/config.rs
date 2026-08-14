//! What a config file means. Cases and their reasoning live in corpus/conformance/config.json.
//!
//! A case's file is a list of lines rather than one string with escapes in it, because a
//! reviewer has to be able to see the TOML they are judging (`docs/testing.md`: cases are
//! text files a reviewer can read).

mod support;

use conformance::Conformance;
use muster_core::config;
use serde_json::{Value, json};
use support::backend::describe_daemon;

#[test]
fn config_conformance() {
    let corpus = Conformance::load("config.json");

    let ran = corpus.run(|given| {
        let text = file(given);
        Ok(match config::parse(&text) {
            Ok(parsed) => {
                json!({ "daemons": parsed.daemons.iter().map(describe_daemon).collect::<Vec<_>>() })
            }
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

fn file(given: &Value) -> String {
    given
        .get("file")
        .and_then(Value::as_array)
        .map(|lines| lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}
