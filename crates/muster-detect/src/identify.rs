//! Which agent a pane's foreground is running.
//!
//! Ported from herdr v0.8.0 `src/detect/mod.rs` (identification) and `src/pane.rs` (the order
//! a probe asks its questions in; Apache-2.0), and changed: names resolve through the loaded
//! manifests rather than a compiled table, a package path is manifest data (`script_paths`)
//! rather than code, and Windows' `cmd` and PowerShell argument unwrapping is gone with
//! Windows.
//!
//! Most agents are not the process the kernel names. They are a script under `node`, `bun` or
//! `python`, a shell script, a Nix wrapper called `.codex-wrapped`, or a runtime that renamed
//! itself. So a process's name is only where the search starts: runtimes are unwrapped to the
//! script they run, argv[0] is consulted, and a symlink is followed to what it points at.

use std::path::Path;

use crate::process::{Job, Process, Processes};
use crate::{Agent, Manifests};

/// What a probe of a pane's foreground found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The group the probe looked at.
    pub group: Option<u32>,
    /// The pane's own shell is in the foreground: whatever ran has returned to it.
    pub shell_in_foreground: bool,
    pub agent: Option<Agent>,
    /// The name the agent was recognised by, for a log line.
    pub process_name: Option<String>,
}

/// Probes a pane's foreground group. A `MUSTER_AGENT` hint on the leader beats its name; the
/// leader's name beats the other members' hints; those beat the best-scoring member's name.
/// The leader alone is asked first because that is one process to read, not all of them.
pub fn probe(
    shell: u32,
    group: Option<u32>,
    processes: &impl Processes,
    manifests: &Manifests,
) -> Probe {
    let hint = |pid| processes.agent_hint(pid).and_then(|name| manifests.agent_named(&name));
    let found = |job: &Job, agent: Agent, process_name: String| Probe {
        group: Some(job.group),
        shell_in_foreground: job.processes.iter().any(|process| process.pid == shell),
        agent: Some(agent),
        process_name: Some(process_name),
    };
    let hinted = |job: &Job, agent: Agent| {
        let name = agent.id().to_string();
        found(job, agent, name)
    };

    let Some(group) = group else {
        return Probe { group: None, shell_in_foreground: false, agent: None, process_name: None };
    };

    if let Some(job) = processes.leader(group) {
        if let Some(agent) = hint(job.group) {
            return hinted(&job, agent);
        }
        if let Some((agent, name)) = identify_in_job(&job, manifests) {
            return found(&job, agent, name);
        }
    }

    let Some(job) = processes.job(shell, group) else {
        return Probe {
            group: Some(group),
            shell_in_foreground: false,
            agent: None,
            process_name: None,
        };
    };
    if let Some(agent) = hint(job.group) {
        return hinted(&job, agent);
    }
    let leader_alone =
        job.leader().map(|leader| Job { group: job.group, processes: vec![leader.clone()] });
    if let Some((agent, name)) = leader_alone.and_then(|leader| identify_in_job(&leader, manifests))
    {
        return found(&job, agent, name);
    }
    if let Some(agent) = job
        .processes
        .iter()
        .filter(|process| process.pid != job.group)
        .find_map(|process| hint(process.pid))
    {
        return hinted(&job, agent);
    }
    let identified = identify_in_job(&job, manifests);
    Probe {
        group: Some(job.group),
        shell_in_foreground: job.processes.iter().any(|process| process.pid == shell),
        agent: identified.as_ref().map(|(agent, _)| agent.clone()),
        process_name: identified.map(|(_, name)| name),
    }
}

/// The agent in a job: its leader if the leader is one, otherwise the member whose name says
/// so most directly - a name found by unwrapping outranks a plain one, and a plain one
/// outranks a runtime or shell that merely happens to be called like an agent.
pub fn identify_in_job(job: &Job, manifests: &Manifests) -> Option<(Agent, String)> {
    if let Some(leader) = job.leader() {
        let candidate = normalized_process_name(leader, manifests);
        if let Some(agent) = manifests.agent_named(&candidate) {
            return Some((agent, candidate));
        }
    }

    let mut best: Option<(u8, Agent, String)> = None;
    for process in &job.processes {
        let candidate = normalized_process_name(process, manifests);
        let Some(agent) = manifests.agent_named(&candidate) else {
            continue;
        };
        let score = process_priority(process, &candidate);
        match &best {
            Some((best_score, _, _)) if *best_score >= score => {}
            _ => best = Some((score, agent, candidate)),
        }
    }
    best.map(|(_, agent, name)| (agent, name))
}

/// The agent one process is, by the names detection reads a foreground by: none for a shell, a
/// hook, or anything else no manifest names.
pub fn identify_process(process: &Process, manifests: &Manifests) -> Option<Agent> {
    manifests.agent_named(&normalized_process_name(process, manifests))
}

/// The name a process should be looked up by.
fn normalized_process_name(process: &Process, manifests: &Manifests) -> String {
    let effective = process.argv0.as_deref().unwrap_or(&process.name);
    let lower = effective.to_lowercase();

    if is_generic_runtime_or_shell(&lower)
        && let Some(wrapped) = wrapped_agent_name(&lower, process.argv.as_deref(), manifests)
    {
        return wrapped;
    }
    if manifests.agent_named(effective).is_some() {
        return effective.to_string();
    }
    let argv0 = process.argv.as_deref().and_then(<[String]>::first);
    if let Some(wrapped) = argv0
        .and_then(|argv0| agent_name_from_path_token(argv0, manifests))
        // The whole command line, split on spaces: a runtime that set its title to
        // "codex --model x" has one argv[0] with spaces in it.
        .or_else(|| {
            let cmdline = process.argv.as_deref()?.join(" ");
            agent_name_from_path_token(cmdline.split_whitespace().next()?, manifests)
        })
    {
        return wrapped;
    }
    effective.to_string()
}

fn wrapped_agent_name(
    runtime: &str,
    argv: Option<&[String]>,
    manifests: &Manifests,
) -> Option<String> {
    let argv = argv?;
    match crate::manifests::lookup_name(path_basename(runtime)).as_str() {
        "node" | "bun" => {
            script_arg_agent_name(argv, &["-e", "--eval", "-p", "--print"], &[], manifests)
        }
        "python" | "python3" => script_arg_agent_name(argv, &["-c"], &["-m"], manifests),
        "sh" | "bash" | "zsh" | "fish" => script_arg_agent_name(argv, &["-c"], &[], manifests),
        _ => None,
    }
}

/// The script a runtime was asked to run, if it was asked to run a script: code passed on the
/// command line (`-e`, `-c`) or a module (`-m`) is not an agent however it is named.
fn script_arg_agent_name(
    argv: &[String],
    eval_flags: &[&str],
    module_flags: &[&str],
    manifests: &Manifests,
) -> Option<String> {
    let mut rest = argv.iter().skip(1);
    while let Some(arg) = rest.next() {
        if arg == "--" {
            return rest.next().and_then(|token| agent_name_from_path_token(token, manifests));
        }
        if flag_matches(arg, eval_flags) || flag_matches(arg, module_flags) {
            return None;
        }
        if arg.starts_with('-') {
            if option_takes_value(arg) {
                let _ = rest.next();
            }
            continue;
        }
        return agent_name_from_path_token(arg, manifests);
    }
    None
}

fn flag_matches(arg: &str, flags: &[&str]) -> bool {
    flags
        .iter()
        .any(|flag| arg == *flag || short_flag_payload(arg, flag) || long_flag_value(arg, flag))
}

/// `-e'code'`: a short flag with its value attached.
fn short_flag_payload(arg: &str, flag: &str) -> bool {
    flag.starts_with('-')
        && !flag.starts_with("--")
        && arg.starts_with(flag)
        && arg.len() > flag.len()
}

/// `--eval=code`.
fn long_flag_value(arg: &str, flag: &str) -> bool {
    flag.starts_with("--") && arg.strip_prefix(flag).is_some_and(|rest| rest.starts_with('='))
}

fn option_takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-r" | "--require"
            | "--loader"
            | "--import"
            | "--experimental-loader"
            | "--inspect-port"
            | "-W"
            | "-X"
            | "-S"
            | "-L"
            | "-o"
    )
}

fn agent_name_from_path_token(token: &str, manifests: &Manifests) -> Option<String> {
    let trimmed = token.trim_matches(|c| matches!(c, '"' | '\''));
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }
    let agent = manifests
        .agent_named(path_basename(trimmed))
        .or_else(|| manifests.agent_for_script_path(trimmed))
        .or_else(|| resolved_agent(trimmed, manifests))?;
    Some(agent.id().to_string())
}

/// Follows a path of two or more components to what it really is: `agent` symlinked to
/// `cursor-agent` is Cursor.
fn resolved_agent(token: &str, manifests: &Manifests) -> Option<Agent> {
    let path = Path::new(token);
    if path.components().count() < 2 {
        return None;
    }
    let resolved = std::fs::canonicalize(path).ok()?;
    manifests.agent_named(resolved.file_name()?.to_str()?)
}

fn path_basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).find(|component| !component.is_empty()).unwrap_or(path)
}

fn process_priority(process: &Process, normalized_name: &str) -> u8 {
    let lower = normalized_name.to_lowercase();
    if lower != process.name.to_lowercase() {
        3
    } else if !is_generic_runtime_or_shell(&lower) {
        2
    } else {
        1
    }
}

fn is_generic_runtime_or_shell(name: &str) -> bool {
    matches!(
        crate::manifests::lookup_name(path_basename(name)).as_str(),
        "sh" | "bash" | "zsh" | "fish" | "tmux" | "node" | "bun" | "python" | "python3"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_shell_flags_do_not_name_a_script() {
        let manifests = Manifests::built_in();
        let argv = ["bash".to_string(), "-lc".to_string()];
        assert_eq!(wrapped_agent_name("bash", Some(&argv), &manifests), None);
    }

    #[test]
    fn a_path_token_needs_an_exact_agent_basename() {
        let manifests = Manifests::built_in();
        assert_eq!(
            agent_name_from_path_token("/nix/store/example/bin/ghcs", &manifests).as_deref(),
            Some("copilot")
        );
        assert_eq!(agent_name_from_path_token("/tmp/my-codex-helper", &manifests), None);
        assert_eq!(agent_name_from_path_token("--codex", &manifests), None);
    }

    #[test]
    fn eval_flags_are_recognised_attached_and_separate() {
        assert!(flag_matches("-e", &["-e"]));
        assert!(flag_matches("-econsole.log(1)", &["-e"]));
        assert!(flag_matches("--eval=1", &["--eval"]));
        assert!(!flag_matches("--evaluate", &["--eval"]));
    }

    #[test]
    fn a_symlinked_argv0_resolves_to_the_agent_it_points_at() {
        let dir =
            std::env::temp_dir().join(format!("muster-detect-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("cursor-agent");
        let link = dir.join("agent");
        std::fs::write(&target, b"#!/bin/sh\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let job = Job {
            group: 42,
            processes: vec![Process {
                pid: 42,
                name: "MainThread".to_string(),
                argv0: None,
                argv: Some(vec![
                    link.to_string_lossy().into_owned(),
                    "--use-system-ca".to_string(),
                    "/tmp/index.js".to_string(),
                ]),
            }],
        };
        let identified = identify_in_job(&job, &Manifests::built_in());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(identified, Some((Agent::new("cursor"), "cursor".to_string())));
    }
}
