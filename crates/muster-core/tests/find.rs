//! Looking for text in a pane's history. Cases live in corpus/conformance/find.json.

use std::fmt::Write as _;

use conformance::{CaseError, Conformance, fields};
use muster_core::find::{Found, Hit, Needle, Reach, found_in};
use muster_core::mirror::backend::Viewport;
use serde_json::{Value, json};

#[test]
fn find_conformance() {
    let corpus = Conformance::load("find.json");

    let ran = corpus.run(|given| {
        let text = given
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| CaseError::new("`text` is missing: there is nothing to search"))?;
        let needle = given
            .get("needle")
            .and_then(Value::as_str)
            .ok_or_else(|| CaseError::new("`needle` is missing: there is nothing to look for"))?;
        let shape = given.get("pane");
        let (viewport, rows_read) = pane(shape, text);

        let found = found_in(text, &Needle::new(needle), viewport, rows_read);
        let reach = found.reach();
        Ok(fields([
            ("hits", Some(json!(found.hits.iter().map(spell).collect::<Vec<_>>()))),
            ("rows_searched", Some(json!(found.rows_searched))),
            // Only for the cases that named a pane to be about. A case about matching says
            // nothing about the reach, and answering for it anyway would make every one of
            // them carry a claim it was not written to make.
            ("reach", shape.map(|_| json!(word(reach)))),
            (
                "rows_held",
                match reach {
                    Reach::Capped { rows_held } => Some(json!(rows_held)),
                    _ => None,
                },
            ),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// The pane a case is about, or the one implied by a case that does not say.
///
/// A case with no `pane` is about matching, and the default has to leave its offsets alone:
/// a viewport exactly as tall as the text, read whole, trims nothing.
fn pane(shape: Option<&Value>, text: &str) -> (Viewport, u32) {
    let Some(shape) = shape else {
        let rows = u32::try_from(muster_core::find::rows_of(text).len()).unwrap_or(u32::MAX);
        return (Viewport { rows_from_bottom: 0, rows, deepest: 0 }, rows);
    };
    let number = |key: &str| {
        u32::try_from(shape.get(key).and_then(Value::as_u64).unwrap_or_default())
            .unwrap_or(u32::MAX)
    };
    (
        Viewport { rows_from_bottom: 0, rows: number("rows"), deepest: number("deepest") },
        number("rows_read"),
    )
}

fn word(reach: Reach) -> &'static str {
    match reach {
        Reach::Whole => "whole",
        Reach::Capped { .. } => "capped",
        Reach::ScreenOnly => "screen_only",
    }
}

/// A hit as the corpus spells it: rows from the bottom, then the bytes it matched.
///
/// One string rather than an object per hit, so a case listing six of them stays a line a
/// reviewer reads rather than a page they scroll.
fn spell(hit: &Hit) -> String {
    format!("{}:{}-{}", hit.rows_from_bottom, hit.matched.start, hit.matched.end)
}

/// A pane holding exactly `rows` rows on a screen of `on_screen`, read whole.
fn holding(rows: u32, on_screen: u32) -> (Viewport, u32) {
    (
        Viewport { rows_from_bottom: 0, rows: on_screen, deepest: rows.saturating_sub(on_screen) },
        rows,
    )
}

#[test]
fn a_hit_can_be_sliced_out_of_the_row_it_names() {
    // The property the byte offsets promise and that no case can state, because the corpus
    // holds ranges rather than the rows they index. A range that split a character would
    // panic here, which is what a consumer showing a result would do.
    let text = "你好 error 世界\nerror\n";
    let (viewport, rows_read) = holding(2, 1);
    let found = found_in(text, &Needle::new("error"), viewport, rows_read);
    let rows: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();

    assert_eq!(found.hits.len(), 2);
    for hit in &found.hits {
        let row = rows[rows.len() - 1 - hit.rows_from_bottom as usize];
        assert_eq!(&row[hit.matched.clone()], "error");
    }
}

#[test]
fn every_row_of_a_full_read_is_reachable_as_an_offset() {
    // A thousand rows is what herdr answers with, so the top one is 999 rows from the
    // bottom and the bottom one is 0. Stated as a property rather than a case because the
    // interesting part is the whole range, and a corpus case holding a thousand rows would
    // be a file nobody reads.
    let mut text = String::new();
    for row in 0..1000 {
        writeln!(text, "row-{row:04}").expect("writing to a String cannot fail");
    }
    let (viewport, rows_read) = holding(1000, 24);
    let found = found_in(&text, &Needle::new("row-"), viewport, rows_read);

    assert_eq!(found.rows_searched, 1000);
    assert_eq!(found.hits.len(), 1000);
    assert_eq!(found.hits.first().map(|hit| hit.rows_from_bottom), Some(0));
    assert_eq!(found.hits.last().map(|hit| hit.rows_from_bottom), Some(999));
}

#[test]
fn nothing_found_is_not_the_same_as_nothing_asked() {
    // Both come back with no hits and the difference is the reach, which is what the bar
    // shows. A search of a thousand rows that found nothing has said something; an empty
    // field has not.
    let (viewport, rows_read) = holding(2, 1);
    let searched = found_in("alpha\nbeta\n", &Needle::new("gamma"), viewport, rows_read);
    let unasked = found_in("alpha\nbeta\n", &Needle::new(""), viewport, rows_read);

    assert_eq!(searched.hits, unasked.hits);
    assert_eq!(searched.rows_searched, 2);
    assert_eq!(unasked.rows_searched, 2);
    assert_eq!(Found::default().rows_searched, 0);
}

#[test]
fn a_pane_nothing_is_known_about_claims_nothing() {
    // What a default `Found` says, which is what a bar draws before anybody has typed. Every
    // other reach is a claim about a pane, and there is no pane here - so `whole` is the one
    // that draws no caveat rather than an assertion that a search happened.
    assert_eq!(Found::default().reach(), Reach::Whole);
}
