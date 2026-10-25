//! What a message's Markdown becomes: the blocks and runs a shell draws. Cases live in
//! corpus/conformance/markdown.json.

use conformance::{CaseError, Conformance};
use muster_core::markdown::{self, Block, Run};
use serde_json::{Map, Value, json};

#[test]
fn markdown_conformance() {
    let corpus = Conformance::load("markdown.json");
    let ran = corpus.run(|given| {
        let source = given
            .get("markdown")
            .and_then(Value::as_str)
            .ok_or_else(|| CaseError::new("`markdown` is missing"))?;
        let blocks = markdown::parse(source);
        Ok(match given.get("lines").and_then(Value::as_u64) {
            None => json!({ "blocks": blocks_json(&blocks) }),
            Some(lines) => {
                let lines =
                    usize::try_from(lines).map_err(|_| CaseError::new("`lines` too big"))?;
                let (kept, cut) = markdown::leading(&blocks, lines);
                json!({ "blocks": blocks_json(&kept), "cut": cut })
            }
        })
    });
    assert_eq!(ran, corpus.cases.len());
}

fn blocks_json(blocks: &[Block]) -> Value {
    Value::Array(blocks.iter().map(block_json).collect())
}

fn block_json(block: &Block) -> Value {
    match block {
        Block::Paragraph(runs) => {
            json!({ "paragraph": runs.iter().map(run_json).collect::<Vec<_>>() })
        }
        Block::Code(code) => json!({ "code": { "language": code.language, "text": code.text } }),
        Block::List(list) => {
            let items: Vec<Value> = list.items.iter().map(|item| blocks_json(item)).collect();
            let mut fields = Map::new();
            if let Some(start) = list.start {
                fields.insert("start".into(), json!(start));
            }
            fields.insert("items".into(), Value::Array(items));
            json!({ "list": fields })
        }
    }
}

/// A run as the corpus spells it: its text, and only the styles it has.
fn run_json(run: &Run) -> Value {
    let mut fields = Map::new();
    fields.insert("text".into(), json!(run.text));
    for (name, on) in [("emphasis", run.emphasis), ("strong", run.strong), ("code", run.code)] {
        if on {
            fields.insert(name.into(), json!(true));
        }
    }
    if let Some(link) = &run.link {
        fields.insert("link".into(), json!(link));
    }
    Value::Object(fields)
}
