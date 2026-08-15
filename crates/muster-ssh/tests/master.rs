//! The command line one remote daemon is reached by. Cases and their reasoning live in
//! corpus/conformance/ssh-master.json.
//!
//! Offline and process-free: what is being judged is a vector of strings, and every option in
//! it exists to make a specific silent failure loud. Whether ssh then connects is the devenv
//! tier's question, and it needs a container.

use conformance::Conformance;
use muster_ssh::{Forward, master_arguments};
use serde_json::{Value, json};

#[test]
fn ssh_master_conformance() {
    let corpus = Conformance::load("ssh-master.json");

    let ran = corpus.run(|given| {
        let forward = Forward {
            host: text(given, "host"),
            options: strings(given, "options"),
            control_path: text(given, "controlPath"),
            local_socket: text(given, "localSocket"),
            remote_socket: text(given, "remoteSocket"),
        };
        Ok(json!({ "arguments": master_arguments(&forward) }))
    });
    assert!(ran > 0, "the ssh master corpus ran no cases, which passes without proving anything");
}

fn text(given: &Value, key: &str) -> String {
    given.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn strings(given: &Value, key: &str) -> Vec<String> {
    given
        .get(key)
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}
