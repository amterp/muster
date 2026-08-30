//! How Muster starts its daemon, and what that decides about permissions.
//! Cases live in corpus/conformance/daemon-launch.json.

use std::collections::BTreeMap;

use conformance::{CaseError, Conformance, fields};
use muster_herdr::daemon::{Launch, carried, launch, open_arguments, output_beside, supplied};
use serde_json::{Value, json};

#[test]
fn daemon_launch_conformance() {
    let corpus = Conformance::load("daemon-launch.json");

    let ran = corpus.run(|given| {
        let binary = given.get("binary").and_then(Value::as_str).ok_or_else(|| {
            CaseError::new("`binary` is missing: there is no path to decide a launch from")
        })?;
        // Where Muster wrote this daemon's config file, which only the shell can answer.
        // Absent is a launch that found nowhere to write one.
        let config = given.get("config").and_then(Value::as_str);

        let mut environment = BTreeMap::new();
        if let Some(raw) = given.get("env").and_then(Value::as_object) {
            for (name, value) in raw {
                environment.insert(name.clone(), value.as_str().unwrap_or_default().to_string());
            }
        }

        let how = launch(binary);
        let output = output_beside(config);

        // The argv only where it exists. A spawned daemon has no `open` line, and emitting an
        // empty one would let a case that meant to check the Launch Services path pass while
        // silently taking the other.
        let opened = match how {
            Launch::ThroughLaunchServices => Some(json!(open_arguments(
                binary,
                output.as_deref(),
                &carried(&environment),
                &supplied(&environment, None, config, None),
            ))),
            Launch::Directly => None,
        };

        Ok(fields([
            (
                "launch",
                Some(json!(match how {
                    Launch::Directly => "directly",
                    Launch::ThroughLaunchServices => "through launch services",
                })),
            ),
            ("output", opened.is_some().then(|| json!(output))),
            ("open", opened),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
