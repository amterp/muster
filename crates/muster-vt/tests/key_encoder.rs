//! The encoder is libghostty-vt's, so these do not test escape-sequence generation -
//! upstream does that, and a second implementation written to agree with it would be the
//! bug. What they pin is everything around it that is ours.
//!
//! Option-as-alt is driven here as one thing rather than two, and that is the point. The
//! encoder's flag alone cannot settle it: a layout that spent option composing a character
//! reports it consumed, and the encoder subtracts consumed modifiers before it ever reads
//! the flag. So `PaneInputSettings::as_alt` rewrites the keystroke first and these cases run
//! both steps together. Running only the second is what let every case here pass while every
//! real keystroke did the opposite.
//!
//! Cases live in corpus/conformance/key-encoder.json.

mod support;

use conformance::{CaseError, Conformance, fields, hex};
use muster_core::input::PaneInputSettings;
use muster_vt::KeyEncoder;
use serde_json::json;
use support::keys::{key_event, profile};

#[test]
fn key_encoder_conformance() {
    let corpus = Conformance::load("key-encoder.json");

    let ran = corpus.run(|given| {
        let profile = profile(given.get("profile"))?;
        let encoder =
            KeyEncoder::new(profile).map_err(|error| CaseError::new(error.to_string()))?;
        // The same setting the encoder was built with, so the two steps cannot disagree -
        // which is exactly the arrangement `PaneInput` holds at runtime.
        let settings =
            PaneInputSettings { option_as_alt: profile.option_acts_as_alt, ..Default::default() };
        let key = key_event(given)?;
        let key = settings.as_alt(&key).unwrap_or(key);
        let bytes = encoder.encode(&key).map_err(|e| CaseError::new(e.to_string()))?;
        Ok(fields([("bytes_hex", Some(json!(hex(&bytes))))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
