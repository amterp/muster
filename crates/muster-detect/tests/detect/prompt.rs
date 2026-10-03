//! Whether a screen is its agent at its prompt, idle or at work, and what the prompt holds, as
//! the doorbell reads it before typing (MIP-4, section 6). Cases and their reasoning live in
//! corpus/conformance/agent-prompt.json.

use conformance::{CaseError, Conformance, fields};
use muster_detect::{Agent, Input, Manifests, Prompt};
use serde_json::{Value, json};

fn prompt(manifests: &Manifests, given: &Value) -> Result<Value, CaseError> {
    let text = |key: &str| given.get(key).and_then(Value::as_str).unwrap_or("");
    let agent = given
        .get("agent")
        .and_then(Value::as_str)
        .ok_or_else(|| CaseError::new("the case names no agent"))?;
    let screen = text("screen");
    let typed = given.get("typed").and_then(Value::as_str).unwrap_or(screen);
    let agent = Agent::new(agent);
    let input = Input { screen, title: text("title"), progress: text("progress") };
    let idle = manifests.prompt(&agent, input, typed);
    let at_work = manifests.prompt_at_work(&agent, input, typed);
    let while_working = at_work.is_some();
    let read = idle.or(at_work);
    Ok(fields([
        ("atPrompt", Some(json!(read.is_some()))),
        ("whileWorking", while_working.then_some(json!(true))),
        (
            "holds",
            match read {
                Some(Prompt::Holds(held)) => Some(json!(held)),
                Some(Prompt::Empty) | None => None,
            },
        ),
    ]))
}

#[test]
fn prompt_conformance() {
    let manifests = Manifests::built_in();
    let corpus = Conformance::load("agent-prompt.json");
    let ran = corpus.run(|given| prompt(&manifests, given));
    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
