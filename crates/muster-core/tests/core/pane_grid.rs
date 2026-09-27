//! How big a pane's grid may be before a frame of it stops fitting. Cases and their reasoning
//! live in corpus/conformance/pane-grid.json.
//!
//! Every case is a sequence, because both answers depend on what was already true: a problem is
//! raised once for a condition that stays true, and cleared when a pane comes back under.

use conformance::{CaseError, Conformance, fields};
use muster_core::PaneKey;
use muster_core::composition::DaemonId;
use muster_core::grid::Grids;
use muster_core::mirror::backend::PaneId;
use serde_json::{Value, json};

#[test]
fn pane_grid_conformance() {
    let corpus = Conformance::load("pane-grid.json");

    let ran = corpus.run(|given| {
        let ceiling = number(given, "ceiling")?;
        let mut grids = Grids::new();
        let (mut raised, mut cleared) = (Vec::new(), Vec::new());
        let mut last: Option<PaneKey> = None;

        for step in given.get("steps").and_then(Value::as_array).into_iter().flatten() {
            let reported = if let Some(pane) = step.get("sized").and_then(Value::as_str) {
                let pane = pane_key(pane)?;
                let reported =
                    grids.sized(&pane, number(step, "columns")?, number(step, "rows")?, ceiling);
                last = Some(pane);
                reported
            } else if let Some(pane) = step.get("forgot").and_then(Value::as_str) {
                let pane = pane_key(pane)?;
                let reported = grids.forget(&pane);
                last = Some(pane);
                reported
            } else {
                return Err(CaseError::new(format!("the step does nothing: {step}")));
            };
            raised.extend(reported.raise.into_iter().map(|(key, _)| json!(key)));
            cleared.extend(reported.clear.into_iter().map(Value::String));
        }

        // The pane the last step was about unless the case names another, so a case with one
        // pane in it does not have to say which one twice.
        let about = match given.get("of").and_then(Value::as_str) {
            Some(pane) => Some(pane_key(pane)?),
            None => last,
        };

        Ok(fields([
            ("raised", Some(Value::Array(raised))),
            ("cleared", Some(Value::Array(cleared))),
            ("may_shrink", Some(json!(about.is_none_or(|pane| grids.may_shrink(&pane, ceiling))))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn number(value: &Value, key: &str) -> Result<u32, CaseError> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| CaseError::new(format!("`{key}` is not a whole number of cells")))
}

/// `local/p1w3r07bsd`, split the way `PaneKey` spells itself: at the first slash, because a
/// daemon id is Muster's own and holds none where a pane id is a name Muster minted.
fn pane_key(spelled: &str) -> Result<PaneKey, CaseError> {
    let (daemon, pane) = spelled.split_once('/').ok_or_else(|| {
        CaseError::new(format!("`{spelled}` is not a pane key - it wants daemon/pane"))
    })?;
    Ok(PaneKey::new(&DaemonId::new(daemon), &PaneId::new(pane)))
}
