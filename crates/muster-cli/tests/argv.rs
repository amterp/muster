//! What a command line means, pinned as data rather than as assertions.
//!
//! Cases live in `corpus/conformance/cli.json`. This is the one part of the CLI that is worth
//! defining language-neutrally: the vocabulary outlives this implementation the way the rest of the
//! corpus does, and a `muster` rewritten in anything else has to turn the same words into the same
//! requests.
//!
//! Three shapes of answer, because a command line has three outcomes. It becomes a request; or it
//! parses and still cannot be carried out, which is a refusal this crate worded; or clap could not
//! read it at all, and then the case pins the *kind* rather than the wording - the text belongs to
//! clap and would change under a version bump, while "an unknown flag is refused rather than
//! swallowed" is the contract.

use std::collections::BTreeMap;

use conformance::{CaseError, Conformance, fields};
use muster_cli::args::{self, Asking};
use muster_proto::{Request, request};
use serde_json::{Value, json};

#[test]
fn cli_conformance() {
    let corpus = Conformance::load("cli.json");

    let ran = corpus.run(|given| {
        let argv: Vec<String> = given
            .get("argv")
            .and_then(Value::as_array)
            .ok_or_else(|| CaseError::new("`argv` is missing: there is no command line to read"))?
            .iter()
            .map(|word| word.as_str().unwrap_or_default().to_string())
            .collect();
        let environment = environment(given);

        Ok(match args::parse(&argv, &environment) {
            Err(args::Failure::Usage(error)) => json!({ "usage": format!("{:?}", error.kind()) }),
            Err(args::Failure::Refused(refusal)) => json!({ "refused": refusal }),
            Ok(invocation) => fields([
                (
                    "request",
                    match &invocation.asking {
                        Asking::Send(request) => Some(described(request)),
                        Asking::Print(_) => None,
                    },
                ),
                ("json", invocation.json.then_some(json!(true))),
                ("socket", invocation.socket.map(|path| json!(path))),
            ]),
        })
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// A completion script comes out of the same definition `--help` does, so it cannot describe a
/// command that is not there.
///
/// Native rather than a corpus case: what a shell needs is that shell's business, and pinning a
/// generated zsh function would be pinning clap_complete rather than anything Muster decides.
#[test]
fn a_shell_can_be_told_how_to_complete_this() {
    let invocation = args::parse(&["completions".to_string(), "zsh".to_string()], &BTreeMap::new())
        .expect("zsh is a shell clap_complete knows");
    let Asking::Print(script) = invocation.asking else {
        panic!("a completion script is printed, not asked of a window - nothing about it needs one")
    };
    for expected in ["#compdef muster", "--socket", "pane", "window"] {
        assert!(
            script.contains(expected),
            "the zsh completion says nothing about {expected:?}, so a shell loading it would \
             complete less than muster takes. Script:\n{script}"
        );
    }
}

fn environment(given: &Value) -> BTreeMap<String, String> {
    given
        .get("env")
        .and_then(Value::as_object)
        .map(|raw| {
            raw.iter()
                .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// The request a command line became, carrying only what it actually said.
///
/// Fields left at their protobuf default are dropped rather than written out as zero, so a case
/// reads as the intention it pins - a `pane new` that named no directory has no `cwd` line, and one
/// that did is a line somebody has to have meant. `side` and `name` are always here: the first
/// because every split has one and the default is the point, and the second because an empty name
/// is what takes a name away, which absence would hide.
fn described(request: &Request) -> Value {
    let Some(payload) = request.payload.as_ref() else {
        return json!("a request with no payload, which nothing here can build");
    };
    match payload {
        request::Payload::ReadWindow(_) => json!({ "read_window": {} }),
        request::Payload::SplitPane(split) => json!({
            "split_pane": fields([
                ("pane_id", said(&split.pane_id)),
                ("side", Some(json!(split.side))),
                ("cwd", said(&split.cwd)),
                ("run", said(&split.run)),
                ("name", said(&split.name)),
                ("take_focus", split.take_focus.then_some(json!(true))),
            ])
        }),
        request::Payload::RenamePane(rename) => json!({
            "rename_pane": fields([
                ("pane_id", said(&rename.pane_id)),
                ("name", Some(json!(rename.name))),
            ])
        }),
        request::Payload::SendToPane(send) => json!({
            "send_to_pane": fields([
                ("pane_id", said(&send.pane_id)),
                ("text", Some(json!(send.text))),
                ("enter", send.enter.then_some(json!(true))),
            ])
        }),
        request::Payload::ClosePane(close) => json!({
            "close_pane": fields([("pane_id", said(&close.pane_id))])
        }),
        request::Payload::FocusPane(focus) => json!({
            "focus_pane": fields([("pane_id", said(&focus.pane_id))])
        }),
        request::Payload::ZoomPane(zoom) => json!({
            "zoom_pane": fields([("pane_id", said(&zoom.pane_id))])
        }),
        request::Payload::FocusTab(focus) => json!({
            "focus_tab": fields([("tab_id", said(&focus.tab_id))])
        }),
        request::Payload::RenameTab(rename) => json!({
            "rename_tab": fields([
                ("tab_id", said(&rename.tab_id)),
                ("name", Some(json!(rename.name))),
            ])
        }),
        other => json!(format!(
            "{other:?}, which this CLI has no way to build - so either the corpus or the parser \
             has grown a request the other has not"
        )),
    }
}

/// A string field, or nothing if the command line did not say.
fn said(value: &str) -> Option<Value> {
    (!value.is_empty()).then(|| json!(value))
}
