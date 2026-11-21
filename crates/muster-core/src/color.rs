/// Parses a person-assigned color. Palette names stay as names so a theme change can recolor
/// the tab or pane.
pub fn parse_color(value: &str) -> Result<Option<String>, &'static str> {
    match value {
        "none" => Ok(None),
        "red" | "green" | "yellow" | "blue" | "magenta" | "cyan" => Ok(Some(value.to_string())),
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
