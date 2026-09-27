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
use muster_vt::{
    Cell, Format, Mode, Palette, Rgb, Row, Screen, ScreenExtras, ScreenFormatOptions, Style,
    Terminal,
};
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

/// A terminal that fell behind, caught up on another's screen, must agree with it on
/// everything but history, and keep the history it had.
///
/// Cases live in corpus/conformance/catch_up.json. `stale` is what the receiver was showing
/// when output stopped reaching it; `feed` is everything the source saw, and `after` is fed
/// to both once caught up.
#[test]
fn catch_up_conformance() {
    let corpus = Conformance::load("catch_up.json");

    let ran = corpus.run(|given| {
        let columns = number(given, "columns")?;
        let rows = number(given, "rows")?;
        let mut source = terminal(columns, rows)?;
        source.write(&history(given));
        source.write(text(given, "feed").as_bytes());

        let mut behind = terminal(columns, rows)?;
        behind.write(text(given, "stale").as_bytes());
        let kept = primary_history(&behind, rows);
        behind.write(&source.catch_up());

        let mut differences = caught_up_differences(&source, &behind, rows, "");
        let now = primary_history(&behind, rows);
        if now != kept {
            differences.push(format!("history: had {kept:?}, now {now:?}"));
        }

        let after = text(given, "after");
        if !after.is_empty() {
            source.write(after.as_bytes());
            behind.write(after.as_bytes());
            differences.extend(caught_up_differences(&source, &behind, rows, "after: "));
        }

        Ok(fields([("differences", Some(json!(differences)))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn caught_up_differences(a: &Terminal, b: &Terminal, rows: u16, prefix: &str) -> Vec<String> {
    let active = |t: &Terminal| {
        let screen = t.screen();
        let first = screen.len().saturating_sub(usize::from(rows));
        screen[first..].to_vec()
    };
    let mut found = state_differences(a, b, prefix);
    found.extend(row_differences(&active(a), &active(b), 0, &format!("{prefix}active ")));
    found
}

/// The primary screen's history as text, whichever screen is active.
fn primary_history(terminal: &Terminal, rows: u16) -> Vec<String> {
    let options = ScreenFormatOptions {
        format: Format::Plain,
        unwrap: false,
        trim: true,
        content: true,
        trailing_blank_rows: true,
        history: true,
        extras: ScreenExtras::default(),
    };
    let text =
        String::from_utf8_lossy(&terminal.format_screen(Screen::Primary, options)).into_owned();
    let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    lines[..lines.len().saturating_sub(usize::from(rows))].to_vec()
}

fn differences_after(original: &Terminal, replayed: &Terminal, given: &Value) -> Vec<String> {
    let nothing_after = given.get("after").is_none() && given.get("theme").is_none();
    if nothing_after { Vec::new() } else { differences(original, replayed, "after: ") }
}

/// Every observable way the two terminals differ, each named so a failure says what.
fn differences(a: &Terminal, b: &Terminal, prefix: &str) -> Vec<String> {
    let mut found = state_differences(a, b, prefix);
    let mut compare = |what: &str, left: String, right: String| {
        if left != right {
            found.push(format!("{prefix}{what}: original {left}, replayed {right}"));
        }
    };
    compare("scrollback rows", a.scrollback_rows().to_string(), b.scrollback_rows().to_string());
    compare("total rows", a.total_rows().to_string(), b.total_rows().to_string());
    found.extend(row_differences(&a.screen(), &b.screen(), 0, prefix));
    found
}

/// Every way the two differ apart from their rows.
fn state_differences(a: &Terminal, b: &Terminal, prefix: &str) -> Vec<String> {
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
    // The alternate screen's only while it is active: a replay recreates no alternate screen a
    // program has left.
    let alternate = a.active_screen() == Screen::Alternate;
    for screen in [Screen::Primary, Screen::Alternate].into_iter().take(1 + usize::from(alternate))
    {
        compare(
            &format!("{screen:?} cursor shape"),
            format!("{:?}", a.cursor_shape_of(screen)),
            format!("{:?}", b.cursor_shape_of(screen)),
        );
    }
    compare("pending wrap", a.pending_wrap().to_string(), b.pending_wrap().to_string());
    for mode in Mode::all() {
        compare(&format!("mode {mode}"), a.mode(mode).to_string(), b.mode(mode).to_string());
    }
    compare(
        "kitty keyboard flags",
        a.kitty_keyboard_flags().to_string(),
        b.kitty_keyboard_flags().to_string(),
    );
    compare("mouse tracking", a.mouse_tracking().to_string(), b.mouse_tracking().to_string());
    compare(
        "mouse mode and format in effect",
        format!("{:?}", a.mouse_in_effect()),
        format!("{:?}", b.mouse_in_effect()),
    );
    compare("title", format!("{:?}", a.title()), format!("{:?}", b.title()));
    compare("pwd", format!("{:?}", a.pwd()), format!("{:?}", b.pwd()));
    compare("foreground", format!("{:?}", a.foreground()), format!("{:?}", b.foreground()));
    compare("background", format!("{:?}", a.background()), format!("{:?}", b.background()));
    compare("cursor color", format!("{:?}", a.cursor_color()), format!("{:?}", b.cursor_color()));
    let (pa, pb) = (a.palette(), b.palette());
    for index in (0..256).filter(|&i| pa[i] != pb[i]) {
        compare(
            &format!("palette {index}"),
            format!("{:?}", pa[index]),
            format!("{:?}", pb[index]),
        );
    }

    found
}

/// Every row that differs, not the first: a case pinning a known gap on one row must still
/// fail when something else breaks further down. Rows are numbered from `first`.
fn row_differences(ra: &[Row], rb: &[Row], first: usize, prefix: &str) -> Vec<String> {
    (0..ra.len().max(rb.len()))
        .filter(|&i| !same_row(ra.get(i), rb.get(i)))
        .map(|i| format!("{prefix}row {}: {}", first + i, row_difference(ra.get(i), rb.get(i))))
        .collect()
}

/// Whether two rows look and behave the same.
///
/// One difference is forgiven, and only this one: a cell with no text against a space, when
/// everything else about the two cells is identical. The formatter writes an unwritten cell
/// before or between text as a space, and a cell an erase painted with a background as a
/// space in that background, and no sequence but the cursor move or erase that made the
/// original can produce the first - so a replay cannot. Both draw the same, copy the same, and
/// read the same through the formatter. And the formatter writes such spaces only on a row
/// that has text elsewhere, so no row turns from blank to not. Style, width, protection and
/// link must still match, which is what keeps this from hiding anything a user could see.
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

fn blank(cell: &Cell) -> bool {
    cell.text.is_empty() || cell.text == " "
}

fn same_cell(a: &Cell, b: &Cell) -> bool {
    if a.text != b.text && !(blank(a) && blank(b)) {
        return false;
    }
    a.width == b.width
        && a.style == b.style
        && a.protected == b.protected
        && a.hyperlink == b.hyperlink
}

/// The first cell that differs in a row and how many do, short enough to read in a case's
/// expectation. The count is what catches a second difference on a row already pinned.
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
    let differing: Vec<usize> = a
        .cells
        .iter()
        .zip(&b.cells)
        .enumerate()
        .filter(|(_, (x, y))| !same_cell(x, y))
        .map(|(column, _)| column)
        .collect();
    let Some(&column) = differing.first() else {
        return format!("{} cells, replayed {}", a.cells.len(), b.cells.len());
    };
    let (x, y) = (&a.cells[column], &b.cells[column]);
    let field = if x.text != y.text && !(blank(x) && blank(y)) {
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
    format!(
        "{:?} vs {:?}, {} of {} cells differ, first cell {column} {field}",
        text(a),
        text(b),
        differing.len(),
        a.cells.len()
    )
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
