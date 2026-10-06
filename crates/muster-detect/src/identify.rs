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
        if let Some(agent) = named_agent(leader, &candidate, manifests) {
            return Some((agent, candidate));
        }
    }

    let mut best: Option<(u8, Agent, String)> = None;
    for process in &job.processes {
        let candidate = normalized_process_name(process, manifests);
        let Some(agent) = named_agent(process, &candidate, manifests) else {
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
/// hook, an agent's helper, or anything else no manifest names.
pub fn identify_process(process: &Process, manifests: &Manifests) -> Option<Agent> {
    named_agent(process, &normalized_process_name(process, manifests), manifests)
}

/// The agent `name` means for `process`, unless the process is one of that agent's helpers: its
/// own binary run under another argv[0] for work that is not a session, as Codex runs its sandbox
/// on Linux, in a process group of its own.
fn named_agent(process: &Process, name: &str, manifests: &Manifests) -> Option<Agent> {
    let agent = manifests.agent_named(name)?;
    let program = process
        .argv0
        .as_deref()
        .or_else(|| process.argv.as_deref()?.first().map(String::as_str))
        .map(path_basename);
    match program {
        Some(program) if manifests.is_helper(&agent, program) => None,
        _ => Some(agent),
    }
}

/// The agent in a job and the arguments its process was started with, after the word that names
/// it: `["--model", "opus"]` for `claude --model opus`, and the same for `node .../claude/cli.js
/// --model opus`, whose runtime and script are how it was run rather than what it was told.
///
/// None when no process is an agent, or the kernel would not give the agent's arguments.
pub fn agent_arguments(job: &Job, manifests: &Manifests) -> Option<(Agent, Vec<String>)> {
    let (agent, _) = identify_in_job(job, manifests)?;
    let names_it = |token: &str| {
        agent_name_from_path_token(token, manifests)
            .and_then(|name| manifests.agent_named(&name))
            .is_some_and(|named| named == agent)
    };
    let process = job
        .leader()
        .into_iter()
        .chain(&job.processes)
        .find(|process| identify_process(process, manifests).as_ref() == Some(&agent))?;
    let argv = process.argv.as_deref()?;
    let at = argv.iter().position(|word| names_it(word)).unwrap_or(0);
    Some((agent, argv.get(at + 1..).unwrap_or_default().to_vec()))
}

/// The path the job's agent was started from, when its first word is one and names the agent
/// itself: `~/.claude/local/claude`, which an alias runs and `PATH` does not hold. None for an
/// agent started by its bare name, or through a runtime or a wrapper script.
pub fn agent_program(job: &Job, manifests: &Manifests) -> Option<String> {
    let (agent, _) = identify_in_job(job, manifests)?;
    let process = job
        .leader()
        .into_iter()
        .chain(&job.processes)
        .find(|process| identify_process(process, manifests).as_ref() == Some(&agent))?;
    let first = process.argv.as_deref()?.first()?;
    let names_it = agent_name_from_path_token(first, manifests)
        .and_then(|name| manifests.agent_named(&name))
        .is_some_and(|named| named == agent);
    (first.contains('/') && names_it).then(|| first.clone())
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

    /// Codex re-runs its own binary as its Linux sandbox, so the kernel names the helper `codex`
    /// and only its argv[0] says otherwise; macOS reports that argv[0] apart, as a basename.
    #[test]
    fn an_agents_helper_is_not_the_agent() {
        let manifests = Manifests::built_in();
        let process = |name: &str, argv0: Option<&str>, argv: &[&str]| Process {
            pid: 7,
            name: name.to_string(),
            argv0: argv0.map(str::to_string),
            argv: Some(argv.iter().map(|word| (*word).to_string()).collect()),
        };
        let helper = process("codex", None, &["codex-linux-sandbox", "--sandbox-policy", "{}"]);
        assert_eq!(identify_process(&helper, &manifests), None);
        let by_path = process("codex", None, &["/tmp/arg0/codex-linux-sandbox", "--", "bash"]);
        assert_eq!(identify_process(&by_path, &manifests), None);
        let on_macos = process("codex", Some("codex-execve-wrapper"), &["codex-execve-wrapper"]);
        assert_eq!(identify_process(&on_macos, &manifests), None);
        let job = Job { group: 7, processes: vec![helper] };
        assert_eq!(identify_in_job(&job, &manifests), None);

        let nested = process("codex", None, &["codex", "exec", "hi"]);
        assert_eq!(identify_process(&nested, &manifests).unwrap().id(), "codex");
        let claude = process("claude", None, &["codex-linux-sandbox"]);
        assert_eq!(
            identify_process(&claude, &manifests).unwrap().id(),
            "claude",
            "a helper is only its own agent's"
        );
    }

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

    fn process(pid: u32, name: &str, argv: &[&str]) -> Process {
        Process {
            pid,
            name: name.to_string(),
            argv0: None,
            argv: Some(argv.iter().map(ToString::to_string).collect()),
        }
    }

    /// The agent's own arguments, whether it is the job's leader, a wrapper's child, or a script
    /// a runtime runs.
    #[test]
    fn an_agents_arguments_follow_the_word_that_names_it() {
        let manifests = Manifests::built_in();
        let alone =
            Job { group: 1, processes: vec![process(1, "claude", &["claude", "--model", "opus"])] };
        assert_eq!(
            agent_arguments(&alone, &manifests),
            Some((Agent::new("claude"), vec!["--model".to_string(), "opus".to_string()]))
        );

        let wrapped = Job {
            group: 1,
            processes: vec![
                process(1, "rad", &["rad", "/bin/ct", "-m", "2"]),
                process(2, "claude", &["/opt/claude", "--model", "opus", "--effort", "high"]),
            ],
        };
        assert_eq!(
            agent_arguments(&wrapped, &manifests).map(|(_, arguments)| arguments.join(" ")),
            Some("--model opus --effort high".to_string()),
            "the wrapper's own -m 2 is not the agent's"
        );

        let scripted = Job {
            group: 1,
            processes: vec![process(
                1,
                "sh",
                &["/bin/sh", "/home/a/bin/claude", "--model", "opus"],
            )],
        };
        assert_eq!(
            agent_arguments(&scripted, &manifests).map(|(_, arguments)| arguments.join(" ")),
            Some("--model opus".to_string())
        );

        let shell = Job { group: 1, processes: vec![process(1, "zsh", &["-zsh"])] };
        assert_eq!(agent_arguments(&shell, &manifests), None);
    }

    /// The path an agent was started from, which is what resumes it, only where its own first
    /// word is that path: a bare name is `PATH`'s to find, and a runtime is not the agent.
    #[test]
    fn an_agents_program_is_its_own_path_and_nothing_else() {
        let manifests = Manifests::built_in();
        let from = |argv: &[&str]| {
            let job = Job { group: 1, processes: vec![process(1, "claude", argv)] };
            agent_program(&job, &manifests)
        };
        assert_eq!(
            from(&["/Users/a/.claude/local/claude", "--model", "opus"]).as_deref(),
            Some("/Users/a/.claude/local/claude")
        );
        assert_eq!(from(&["claude", "--model", "opus"]), None, "a bare name is PATH's");
        let scripted = Job {
            group: 1,
            processes: vec![process(1, "sh", &["/bin/sh", "/home/a/bin/claude", "--model"])],
        };
        assert_eq!(agent_program(&scripted, &manifests), None, "the runtime is not the agent");
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
