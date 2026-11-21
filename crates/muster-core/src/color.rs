const PERSONAL_HUE_NAMES: &[&str] = &["red", "green", "yellow", "blue", "magenta", "cyan"];

/// Parses a person-assigned color. Palette names stay as names so a theme change can recolor
/// the tab or pane.
pub fn parse_color(value: &str) -> Result<Option<String>, &'static str> {
    match value {
        "none" => Ok(None),
        _ if PERSONAL_HUE_NAMES.contains(&value) => Ok(Some(value.to_string())),
        _ if value.strip_prefix('#').is_some_and(|hex| {
            hex.len() == 6 && hex.bytes().all(|digit| digit.is_ascii_hexdigit())
        }) =>
        {
            Ok(Some(value.to_ascii_lowercase()))
        }
        _ => Err("choose red, green, yellow, blue, magenta, cyan, #rrggbb, or none"),
    }
}

/// The seam carries a clear as an empty string.
pub fn parse_color_request(value: &str) -> Result<Option<String>, &'static str> {
    parse_color(if value.is_empty() { "none" } else { value })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TabColor {
    pub generation: u64,
    pub value: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{PERSONAL_HUE_NAMES, parse_color_request};
    use conformance::{CaseError, Conformance};
    use serde_json::json;

    #[test]
    fn personal_color_conformance() {
        let mut corpus = Conformance::load("personal-colors.json");
        let mut names = Vec::new();
        for case in &mut corpus.cases {
            let expected = case.expect.as_object_mut().expect("a color expectation object");
            if expected.remove("ansiIndex").is_some_and(|index| index.is_number()) {
                names.push(case.given.as_str().expect("a palette name"));
            }
            expected.remove("hex");
        }
        assert_eq!(PERSONAL_HUE_NAMES, names.as_slice(), "named colors in {}", corpus.file);

        let ran = corpus.run(|given| {
            let value = given.as_str().ok_or_else(|| CaseError::new("expected a color string"))?;
            let result = parse_color_request(value);
            Ok(json!({ "accepted": result.is_ok(), "stored": result.ok().flatten() }))
        });
        assert!(ran > 0);
    }
}
