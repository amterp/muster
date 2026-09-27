//! A replayed terminal must be indistinguishable from the one it was replayed from.
//!
//! The oracle is libghostty-vt itself: feed a case's bytes to terminal A, replay A into a
//! fresh terminal B, and compare everything either one can be asked - every row of the
//! screen with its styles, the cursor, every mode, the colors - and then feed both the same
//! bytes afterwards and compare again. Nothing here states what a replay's bytes should be,
//! so the cases outlive any one way of composing it (MIP-3 sections 5 and 13).
//!
//! Cases live in corpus/conformance/replay.json.

use conformance::{CaseError, Conformance, fields};
use muster_vt::{Cell, Mode, Palette, Rgb, Row, Style, Terminal};
use serde_json::{Value, json};

#[test]
fn replay_conformance() {
    let corpus = Conformance::load("replay.json");

    let ran = corpus.run(|given| {
        let columns = number(given, "columns")?;
        let rows = number(given, "rows")?;
        let mut original = terminal(columns, rows)?;
        original.write(&history(given));
        original.write(text(given, "feed").as_bytes());

        let mut replayed = terminal(columns, rows)?;
        replayed.write(&original.replay());

        let mut differences = differences(&original, &replayed, "");

        if let Some(theme) = given.get("theme") {
            let palette = theme_palette(theme)?;
            original.set_default_palette(&palette);
            replayed.set_default_palette(&palette);
        }
        let after = text(given, "after");
        original.write(after.as_bytes());
        replayed.write(after.as_bytes());
        differences.extend(differences_after(&original, &replayed, given));

        Ok(fields([("differences", Some(json!(differences)))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn differences_after(original: &Terminal, replayed: &Terminal, given: &Value) -> Vec<String> {
    let nothing_after = given.get("after").is_none() && given.get("theme").is_none();
    if nothing_after { Vec::new() } else { differences(original, replayed, "after: ") }
}

/// Every observable way the two terminals differ, each named so a failure says what.
fn differences(a: &Terminal, b: &Terminal, prefix: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut compare = |what: &str, left: String, right: String| {
        if left != right {
            found.push(format!("{prefix}{what}: original {left}, replayed {right}"));
        }
    };

    compare(
        "active screen",
        format!("{:?}", a.active_screen()),
        format!("{:?}", b.active_screen()),
    );
    compare("cursor", format!("{:?}", a.cursor()), format!("{:?}", b.cursor()));
    compare("pending wrap", a.pending_wrap().to_string(), b.pending_wrap().to_string());
    compare("scrollback rows", a.scrollback_rows().to_string(), b.scrollback_rows().to_string());
    compare("total rows", a.total_rows().to_string(), b.total_rows().to_string());
    for mode in Mode::all() {
        compare(&format!("mode {mode}"), a.mode(mode).to_string(), b.mode(mode).to_string());
    }
    compare(
        "kitty keyboard flags",
        a.kitty_keyboard_flags().to_string(),
        b.kitty_keyboard_flags().to_string(),
    );
    compare("mouse tracking", a.mouse_tracking().to_string(), b.mouse_tracking().to_string());
    compare("title", format!("{:?}", a.title()), format!("{:?}", b.title()));
    compare("pwd", format!("{:?}", a.pwd()), format!("{:?}", b.pwd()));
    compare("foreground", format!("{:?}", a.foreground()), format!("{:?}", b.foreground()));
    compare("background", format!("{:?}", a.background()), format!("{:?}", b.background()));
    compare("cursor color", format!("{:?}", a.cursor_color()), format!("{:?}", b.cursor_color()));
    let (pa, pb) = (a.palette(), b.palette());
    if let Some(index) = (0..256).find(|&i| pa[i] != pb[i]) {
        compare(
            &format!("palette {index}"),
            format!("{:?}", pa[index]),
            format!("{:?}", pb[index]),
        );
    }

    let (ra, rb) = (a.screen(), b.screen());
    if let Some(index) = (0..ra.len().max(rb.len())).find(|&i| !same_row(ra.get(i), rb.get(i))) {
        found
            .push(format!("{prefix}row {index}: {}", row_difference(ra.get(index), rb.get(index))));
    }
    found
}

/// Whether two rows look and behave the same.
///
/// One difference is forgiven, and only this one: a cell an erase painted with a background
/// and no text, against a space in the same style. The formatter writes the first as the
/// second, both draw identically, and no VT sequence other than the erase that made it can
/// produce the first - so a replay cannot, and a test that demanded it would demand nothing
/// a user could see.
fn same_row(a: Option<&Row>, b: Option<&Row>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            a.wraps == b.wraps
                && a.cells.len() == b.cells.len()
                && a.cells.iter().zip(&b.cells).all(|(x, y)| same_cell(x, y))
        }
        (None, None) => true,
        _ => false,
    }
}

fn same_cell(a: &Cell, b: &Cell) -> bool {
    let painted_blank = |cell: &Cell| cell.text.is_empty() || cell.text == " ";
    if a.text != b.text && !(painted_blank(a) && painted_blank(b) && a.style.background.is_some()) {
        return false;
    }
    a.width == b.width
        && a.style == b.style
        && a.protected == b.protected
        && a.hyperlink == b.hyperlink
}

/// The first thing that differs in a row, short enough to read in a case's expectation.
fn row_difference(a: Option<&Row>, b: Option<&Row>) -> String {
    let (Some(a), Some(b)) = (a, b) else {
        return format!(
            "present in {} only",
            if a.is_some() { "the original" } else { "the replay" }
        );
    };
    let text = |row: &Row| row.text().trim_end().to_string();
    if a.wraps != b.wraps {
        return format!("{:?} wraps={}, replayed wraps={}", text(a), a.wraps, b.wraps);
    }
    let Some((column, (x, y))) =
        a.cells.iter().zip(&b.cells).enumerate().find(|(_, (x, y))| !same_cell(x, y))
    else {
        return format!("{} cells, replayed {}", a.cells.len(), b.cells.len());
    };
    let field = if x.text != y.text {
        format!("text {:?}, replayed {:?}", x.text, y.text)
    } else if x.width != y.width {
        format!("width {:?}, replayed {:?}", x.width, y.width)
    } else if x.style != y.style {
        style_difference(&x.style, &y.style)
    } else if x.protected != y.protected {
        format!("protected {}, replayed {}", x.protected, y.protected)
    } else {
        format!("hyperlink {:?}, replayed {:?}", x.hyperlink, y.hyperlink)
    };
    format!("{:?} vs {:?}, cell {column} {field}", text(a), text(b))
}

/// Only the style fields that differ, since a whole style is a line of noise.
fn style_difference(a: &Style, b: &Style) -> String {
    let fields = [
        ("foreground", format!("{:?}", a.foreground), format!("{:?}", b.foreground)),
        ("background", format!("{:?}", a.background), format!("{:?}", b.background)),
        ("underline color", format!("{:?}", a.underline_color), format!("{:?}", b.underline_color)),
        ("bold", a.bold.to_string(), b.bold.to_string()),
        ("italic", a.italic.to_string(), b.italic.to_string()),
        ("faint", a.faint.to_string(), b.faint.to_string()),
        ("blink", a.blink.to_string(), b.blink.to_string()),
        ("inverse", a.inverse.to_string(), b.inverse.to_string()),
        ("invisible", a.invisible.to_string(), b.invisible.to_string()),
        ("strikethrough", a.strikethrough.to_string(), b.strikethrough.to_string()),
        ("overline", a.overline.to_string(), b.overline.to_string()),
        ("underline", a.underline.to_string(), b.underline.to_string()),
    ];
    fields
        .iter()
        .filter(|(_, x, y)| x != y)
        .map(|(name, x, y)| format!("{name} {x}, replayed {y}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn terminal(columns: u16, rows: u16) -> Result<Terminal, CaseError> {
    Terminal::new(columns, rows).map_err(|error| CaseError::new(error.to_string()))
}

/// `history` numbered lines, fed before the case's own bytes, so a case can put rows above
/// the screen without spelling each one out.
fn history(given: &Value) -> Vec<u8> {
    let count = given.get("history").and_then(Value::as_u64).unwrap_or(0);
    (0..count).flat_map(|n| format!("history {n}\r\n").into_bytes()).collect()
}

/// A theme switch: the app's palette with every entry moved, so an entry that failed to
/// follow it shows up.
fn theme_palette(theme: &Value) -> Result<Palette, CaseError> {
    let shift = theme.as_u64().ok_or_else(|| CaseError::new("theme is a number to shift by"))?;
    let shift = u8::try_from(shift).map_err(|_| CaseError::new("theme fits in a byte"))?;
    let mut palette = [Rgb::default(); 256];
    for (index, entry) in palette.iter_mut().enumerate() {
        let index = u8::try_from(index).expect("256 entries");
        *entry = Rgb { r: index.wrapping_add(shift), g: shift, b: index };
    }
    Ok(palette)
}

fn number(given: &Value, key: &str) -> Result<u16, CaseError> {
    given
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| CaseError::new(format!("given.{key} is a grid dimension")))
}

fn text(given: &Value, key: &str) -> String {
    given.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}
