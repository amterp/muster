//! Choosing, naming and checking Muster's own daemon for a machine that is not this one.
//! Cases live in corpus/conformance/remote-daemon.json.

use conformance::{CaseError, Conformance, fields};
use muster_herdr::remote::{asset_name, cached_at, digest, installed_at, muster_home};
use muster_ssh::Platform;
use serde_json::{Value, json};

#[test]
fn remote_daemon_conformance() {
    let corpus = Conformance::load("remote-daemon.json");

    let ran = corpus.run(|given| {
        if let Some(asked) = given.get("asset_for") {
            let platform =
                Platform { system: text(asked, "system")?, machine: text(asked, "machine")? };
            return Ok(match asset_name(&platform, &text(asked, "host")?) {
                Ok(asset) => fields([("asset", Some(json!(asset)))]),
                // Null rather than absent: "there is no asset for this machine" is the
                // answer these cases are about, not a field that does not apply.
                Err(refusal) => {
                    fields([("asset", Some(Value::Null)), ("refused", Some(json!(refusal)))])
                }
            });
        }

        if let Some(asked) = given.get("muster_home_from") {
            let environment = environment(asked)?;
            return Ok(fields([(
                "muster_home",
                Some(muster_home(&environment).map_or(Value::Null, |home| json!(home))),
            )]));
        }

        if let Some(asked) = given.get("installed_at") {
            let path = installed_at(&text(asked, "muster_home")?, &text(asked, "version")?);
            return Ok(fields([("path", Some(json!(path)))]));
        }

        if let Some(asked) = given.get("cached_at") {
            let path =
                cached_at(&text(asked, "cache")?, &text(asked, "version")?, &text(asked, "asset")?);
            return Ok(fields([("path", Some(json!(path)))]));
        }

        if let Some(bytes) = given.get("digest_of").and_then(Value::as_str) {
            return Ok(fields([("sha256", Some(json!(digest(bytes.as_bytes()))))]));
        }

        Err(CaseError::new(
            "no `asset_for`, `muster_home_from`, `installed_at`, `cached_at` or `digest_of`: \
             there is nothing to ask",
        ))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn text(given: &Value, key: &str) -> Result<String, CaseError> {
    given
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CaseError::new(format!("`{key}` is missing, or is not a string")))
}

fn environment(given: &Value) -> Result<std::collections::BTreeMap<String, String>, CaseError> {
    let raw =
        given.as_object().ok_or_else(|| CaseError::new("the environment is not an object"))?;
    Ok(raw
        .iter()
        .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect())
}
