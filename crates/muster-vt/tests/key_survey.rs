//! The survey: what Muster puts on a pane's input for the keys people press constantly.
//!
//! One matrix with one reason rather than nineteen behaviors, and its oracle is upstream,
//! so it stays a rendered snapshot instead of becoming conformance cases with nineteen
//! manufactured justifications.
//!
//! The keystrokes and the profiles come from the `survey` section of
//! `corpus/conformance/key-encoder.json`; what stays here is the rendering, which is the
//! snapshot's format rather than the corpus's. A list left in this file is one the next
//! language re-types, and then a snapshot both languages agree on says nothing about it -
//! the same failure as a snapshot re-recorded to make a rewrite pass.

mod support;

use std::fmt::Write as _;

use conformance::Conformance;
use muster_core::input::{KeyEvent, TerminalModeProfile};
use muster_vt::KeyEncoder;
use serde_json::Value;
use support::expect_snapshot;
use support::keys::{key_event, named_profile};

/// What the keys people press constantly put on a pane, under every profile the survey names.
///
/// One test rather than one per profile, because the corpus decides how many profiles there
/// are: a third added there and not here would be data nothing reads, which is the
/// silently-skipped suite in its newest costume.
///
/// `herdrTUI` is not reachable today - it needs mode state herdr does not expose - and is
/// rendered anyway, because the difference between the two files is exactly what the upstream
/// ask is worth.
#[test]
fn what_the_keys_people_press_constantly_put_on_a_pane() {
    let corpus = Conformance::load("key-encoder.json");
    let survey = corpus.survey.as_ref().expect("key-encoder.json carries a `survey` section");
    let keystrokes = keystrokes(survey);
    let snapshots = survey["snapshots"].as_array().expect("`snapshots` is a list");
    assert!(!snapshots.is_empty(), "a survey that renders nothing verifies nothing");

    for snapshot in snapshots {
        let name = snapshot["profile"].as_str().expect("a snapshot names its profile");
        let profile = named_profile(name).expect("the survey names a profile the encoder has");
        let file = snapshot["file"].as_str().expect("a snapshot names its file");
        expect_snapshot(&render(profile, &keystrokes), file);
    }
}

/// The matrix, as the corpus states it: a label and the keystroke it stands for.
fn keystrokes(survey: &Value) -> Vec<(String, KeyEvent)> {
    let listed = survey["keystrokes"].as_array().expect("`keystrokes` is a list");
    assert!(!listed.is_empty(), "a survey of nothing renders an empty file and passes");
    listed
        .iter()
        .map(|entry| {
            let name = entry["name"].as_str().expect("a keystroke carries its label").to_string();
            let key = key_event(entry)
                .unwrap_or_else(|error| panic!("the survey's `{name}` does not read: {error}"));
            (name, key)
        })
        .collect()
}

fn render(profile: TerminalModeProfile, keystrokes: &[(String, KeyEvent)]) -> String {
    let encoder = KeyEncoder::new(profile).expect("libghostty-vt should give us an encoder");
    let width = keystrokes.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let mut out = String::new();
    for (name, event) in keystrokes {
        let bytes = encoder.encode(event).expect("every keystroke here encodes");
        let _ = writeln!(out, "{name:width$}  {}", readable(&bytes));
    }
    out
}

/// Bytes as a reviewer reads them: ESC for 0x1b, ^C for control characters, printable
/// characters as themselves. A hex dump would be exact and unreadable, and nobody would
/// notice the day it changed.
fn readable(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(nothing)".to_string();
    }
    bytes
        .iter()
        .map(|byte| match byte {
            0x1b => "ESC".to_string(),
            0x7f => "DEL".to_string(),
            0x00..=0x1f => format!("^{}", (byte + 0x40) as char),
            0x20 => "SP".to_string(),
            other => (*other as char).to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}
