//! `muster harness install`: a harness's adapter, installed through the harness's own command.
//!
//! Never by writing into a harness's configuration: a person's harness config may be managed by
//! a tool of their own that rewrites it, and the harness's command is the one way in that both
//! agree on. What no command of the harness's can do is printed for the person to do by hand.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use muster_core::harnesses::{self, Harness};
use serde_json::json;

use crate::Trouble;
use crate::environment::muster_home;
use crate::render::shell_word;

/// What installing one harness's adapter takes: the harness's own commands, run in order, and
/// what is left for the person afterwards, a line each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub commands: Vec<Vec<String>>,
    pub then: Vec<String>,
}

impl Plan {
    fn printed(&self) -> Vec<String> {
        self.commands.iter().map(|command| line(command)).collect()
    }
}

/// The harness a person typed, or a refusal naming the ones there are.
pub fn harness(name: &str) -> Result<Harness, String> {
    harnesses::named(name).ok_or_else(|| {
        let known: Vec<&str> = harnesses::WITH_ADAPTERS.iter().map(|harness| harness.dir).collect();
        format!(
            "Muster ships no adapter for {name:?}, so there is nothing to install. It has one for \
             {}; every other harness is read off its screen (`muster docs harnesses`).",
            known.join(", ")
        )
    })
}

/// What installing `harness`'s adapter from `extras` takes.
pub fn plan(harness: Harness, extras: &Path) -> Plan {
    let readme = shell_word(&extras.join(harness.dir).join("README.md").display().to_string());
    let words = |words: &[&str]| words.iter().map(ToString::to_string).collect::<Vec<_>>();
    let extras_path = extras.display().to_string();
    match harness.dir {
        "claude-code" => Plan {
            commands: vec![
                words(&["claude", "plugin", "marketplace", "add", &extras_path]),
                words(&["claude", "plugin", "install", "muster@muster"]),
            ],
            then: vec![
                "Claude Code takes a statusline only from its settings, which Muster does not \
                 edit. To report context, model and cost, and to name the pane after the \
                 session, add this to your settings yourself:"
                    .to_string(),
                format!(
                    "  \"statusLine\": {{\"type\": \"command\", \"refreshInterval\": 2, \
                     \"command\": {}}}",
                    json!(extras.join("claude-code").join("statusline.sh").display().to_string())
                ),
                "Sessions already running take the plugin up when they restart.".to_string(),
                format!("{readme} has the statusline and the messaging hooks."),
            ],
        },
        "codex" => Plan {
            commands: vec![
                words(&["codex", "plugin", "marketplace", "add", &extras_path]),
                words(&["codex", "plugin", "add", "muster-codex@muster"]),
            ],
            then: vec![
                "Codex runs a hook only once you trust it: review them in /hooks in a session."
                    .to_string(),
                "Sessions already running take the plugin up when they restart.".to_string(),
                format!("{readme} has the messaging hooks and what its sandbox needs."),
            ],
        },
        _ => Plan {
            commands: Vec::new(),
            then: vec![
                "OpenCode installs plugins from npm, not from a file, and Muster does not write \
                 into its config folder, so this runs nothing. Link the plugin yourself:"
                    .to_string(),
                "  mkdir -p ~/.config/opencode/plugin".to_string(),
                format!(
                    "  ln -sf {} ~/.config/opencode/plugin/muster.js",
                    shell_word(&extras.join("opencode/plugin/muster.js").display().to_string())
                ),
                format!("{readme} has what it reports."),
            ],
        },
    }
}

/// Where this machine's adapters are. A bundle's own first, since its path outlives an update;
/// then `$MUSTER_HOME/extras` where an install put a link there, as Muster's install on an SSH
/// machine does, so that a path handed to a harness outlives the version it came with; otherwise
/// beside this executable. The link comes after a bundle because an SSH install also makes it on
/// a Mac with an app of its own, which may be another version.
pub fn extras(
    environment: &BTreeMap<String, String>,
    executable: Option<PathBuf>,
) -> Result<PathBuf, Trouble> {
    let executable = executable.and_then(|path| path.canonicalize().ok());
    let beside = executable.as_deref().map(harnesses::extras_beside);
    if let Some(bundled) = beside
        .as_ref()
        .filter(|extras| extras.ends_with("Contents/Resources/extras") && extras.is_dir())
    {
        return Ok(bundled.clone());
    }
    if let Some(linked) = muster_home(environment).map(|home| Path::new(&home).join("extras"))
        && linked.is_dir()
    {
        return Ok(linked);
    }
    match beside {
        Some(extras) if extras.is_dir() => Ok(extras),
        _ => Err(Trouble::Refused(format!(
            "this muster{} has no adapters beside it, so there is nothing to install from. The \
             muster in Muster.app carries them, and so does one Muster installed on an SSH \
             machine; from a source checkout, the adapters are its `extras/`, and \
             `muster docs harnesses` has the commands to run against it.",
            executable.map(|path| format!(" at {}", path.display())).unwrap_or_default()
        ))),
    }
}

/// Installs `harness`'s adapter, or with `dry_run` prints what that would run.
pub fn install(
    harness: Harness,
    dry_run: bool,
    json: bool,
    environment: &BTreeMap<String, String>,
    out: &mut impl Write,
    errors: &mut impl Write,
) -> Result<(), Trouble> {
    let extras = extras(environment, std::env::current_exe().ok())?;
    let plan = plan(harness, &extras);
    let printed = |out: &mut dyn Write, ran: bool| {
        let _ = if json {
            writeln!(
                out,
                "{}",
                json!({
                    "harness": harness.dir,
                    "extras": extras.display().to_string(),
                    "commands": plan.printed(),
                    "ran": ran,
                    "then": plan.then,
                })
            )
        } else if !dry_run {
            writeln!(out, "{}", plan.then.join("\n"))
        } else {
            // Commented, so the whole answer pastes into a shell and runs only the commands.
            let then: Vec<String> = plan.then.iter().map(|line| format!("# {line}")).collect();
            writeln!(
                out,
                "{}",
                plan.printed().into_iter().chain(then).collect::<Vec<_>>().join("\n")
            )
        };
    };
    if dry_run || plan.commands.is_empty() {
        printed(out, false);
        return Ok(());
    }
    for (done, command) in plan.commands.iter().enumerate() {
        if !json {
            let _ = writeln!(out, "$ {}", line(command));
        }
        let left = || plan.printed()[done..].join("\n  ");
        let ran = Command::new(&command[0])
            .args(&command[1..])
            .env_clear()
            .envs(environment)
            .output()
            .map_err(|error| {
                Trouble::Refused(format!(
                    "`{}` could not be run ({error}), so {}'s adapter is not installed. Install \
                     {} or put it on this PATH, then run:\n  {}",
                    command[0],
                    harness.name,
                    harness.name,
                    left()
                ))
            })?;
        let _ = if json { errors.write_all(&ran.stdout) } else { out.write_all(&ran.stdout) };
        let _ = errors.write_all(&ran.stderr);
        if !ran.status.success() {
            return Err(Trouble::Refused(format!(
                "`{}` failed ({}), so {}'s adapter is not installed; what it said is above. If it \
                 says a `muster` marketplace is already added from somewhere else, such as a \
                 checkout of Muster's source, `{} plugin marketplace remove muster` makes room \
                 for this one. Then run what is left:\n  {}",
                line(command),
                ran.status,
                harness.name,
                command[0],
                left()
            )));
        }
    }
    printed(out, true);
    Ok(())
}

fn line(command: &[String]) -> String {
    command.iter().map(|word| shell_word(word)).collect::<Vec<_>>().join(" ")
}
