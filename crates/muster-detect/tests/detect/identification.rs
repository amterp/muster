//! Which agent a foreground job is, from the facts the kernel gives about it. Cases and their
//! reasoning live in corpus/conformance/agent-identification.json.

use std::collections::HashMap;

use conformance::{CaseError, Conformance, fields};
use muster_detect::{Job, Manifests, Process, Processes, probe};
use serde_json::{Value, json};

/// A foreground job as a case describes it.
struct Given {
    job: Job,
    hints: HashMap<u32, String>,
}

impl Processes for Given {
    fn leader(&self, group: u32) -> Option<Job> {
        let leader = self.job.processes.iter().find(|process| process.pid == group)?;
        Some(Job { group, processes: vec![leader.clone()] })
    }

    fn job(&self, _shell: u32, group: u32) -> Option<Job> {
        (group == self.job.group).then(|| self.job.clone())
    }

    fn agent_hint(&self, pid: u32) -> Option<String> {
        self.hints.get(&pid).cloned()
    }
}

fn number(value: &Value, what: &str) -> Result<u32, CaseError> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| CaseError::new(format!("{what} is not a pid")))
}

fn given(value: &Value) -> Result<Given, CaseError> {
    let group = number(&value["group"], "group")?;
    let processes = value["processes"]
        .as_array()
        .ok_or_else(|| CaseError::new("processes is not a list"))?
        .iter()
        .map(|process| {
            Ok(Process {
                pid: number(&process["pid"], "a process's pid")?,
                name: process["name"].as_str().unwrap_or_default().to_string(),
                argv0: process["argv0"].as_str().map(str::to_string),
                argv: process["argv"].as_array().map(|argv| {
                    argv.iter().filter_map(Value::as_str).map(str::to_string).collect()
                }),
            })
        })
        .collect::<Result<_, CaseError>>()?;
    let hints = value["hints"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(pid, name)| Some((pid.parse().ok()?, name.as_str()?.to_string())))
        .collect();
    Ok(Given { job: Job { group, processes }, hints })
}

#[test]
fn identification_conformance() {
    let manifests = Manifests::built_in();
    let corpus = Conformance::load("agent-identification.json");
    let ran = corpus.run(|value| {
        let given = given(value)?;
        // No process has pid 0, so a shell is only in the foreground when a case says it is.
        let shell = if value["shell"].is_null() { 0 } else { number(&value["shell"], "shell")? };
        let found = probe(shell, Some(given.job.group), &given, &manifests);
        Ok(fields([
            ("agent", found.agent.as_ref().map(|agent| json!(agent.id()))),
            (
                "processName",
                found.agent.as_ref().and(found.process_name.as_ref()).map(|name| json!(name)),
            ),
            ("shellInForeground", found.shell_in_foreground.then_some(json!(true))),
        ]))
    });
    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
