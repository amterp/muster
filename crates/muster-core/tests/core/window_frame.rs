//! Where a window opens when the screens have moved on. Cases live in
//! corpus/conformance/window-frame.json.

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::presentation::Frame;
use serde_json::{Value, json};

#[test]
fn window_frame_conformance() {
    let corpus = Conformance::load("window-frame.json");

    let ran = corpus.run(|given| {
        let frame = rect(given.get("frame"))?;
        let screens = given
            .get("screens")
            .and_then(Value::as_array)
            .ok_or_else(|| CaseError::new("the case names no `screens` list"))?
            .iter()
            .map(|screen| rect(Some(screen)))
            .collect::<Result<Vec<Frame>, CaseError>>()?;

        Ok(fields([("frame", Some(json!(spell(frame.fitted(&screens)))))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// A rectangle as a case writes one.
fn rect(value: Option<&Value>) -> Result<Frame, CaseError> {
    let table = value.ok_or_else(|| CaseError::new("the case names no rectangle"))?;
    let number = |key: &str| {
        table
            .get(key)
            .and_then(Value::as_f64)
            .ok_or_else(|| CaseError::new(format!("a rectangle in this case has no `{key}`")))
    };
    Ok(Frame {
        x: number("x")?,
        y: number("y")?,
        width: number("width")?,
        height: number("height")?,
    })
}

/// One line a reader can compare against the case they are looking at, rather than four fields
/// a failure would print separately.
fn spell(frame: Frame) -> String {
    format!("{},{} {}x{}", frame.x, frame.y, frame.width, frame.height)
}
