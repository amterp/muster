//! `muster harness install`: a harness's adapter, installed through the harness's own command.
//!
//! Never by writing into a harness's configuration: a person's harness config may be managed by
//! a tool of their own that rewrites it, and the harness's command is the one way in that both
//! agree on. What no command of the harness's can do is printed for the person to do by hand.

use std::collections::BTreeMap;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use muster_core::harnesses::{self, Harness, Kind};
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
    match harness.kind {
        Kind::ClaudeCode => Plan {
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
                    json!(shell_word(
                        &extras.join("claude-code").join("statusline.sh").display().to_string()
                    ))
                ),
                "Sessions already running take the plugin up when they restart.".to_string(),
                format!("{readme} has the statusline and the messaging hooks."),
            ],
        },
        Kind::Codex => Plan {
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
        Kind::OpenCode => Plan {
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
        let mut running = Command::new(&command[0]);
        running.args(&command[1..]).env_clear().envs(environment);
        let ran = run_within(&mut running, PATIENCE)
            .map_err(|error| {
                Trouble::Refused(format!(
                    "`{}` could not be run ({error}), so {}'s adapter is not installed. Install \
                     {} or put it on this PATH, then run:\n  {}",
                    command[0],
                    harness.name,
                    harness.name,
                    left()
                ))
            })?
            .ok_or_else(|| {
                Trouble::Refused(format!(
                    "`{}` had not finished after {}s and was stopped, so {}'s adapter may be \
                     only partly installed. A harness asking a question on its terminal, or \
                     waiting on the network, holds a command this long; run what is left \
                     yourself, where you can see it:\n  {}",
                    line(command),
                    PATIENCE.as_secs(),
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

/// How long one of a harness's install commands may take. A local plugin installs in a second or
/// two, so this is far past anything but a command waiting on something that will not come.
const PATIENCE: Duration = Duration::from_mins(2);

/// Runs `command` to its end and says what it printed, or stops it and says nothing once it has
/// run for `limit`.
fn run_within(command: &mut Command, limit: Duration) -> std::io::Result<Option<Output>> {
    let mut child =
        command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let began = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if began.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stderr) = (stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default());
    Ok(status.map(|status| Output { status, stdout, stderr }))
}

fn line(command: &[String]) -> String {
    command.iter().map(|word| shell_word(word)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_that_does_not_finish_in_time_is_stopped() {
        let began = Instant::now();
        let ran = run_within(
            Command::new("/bin/sh").args(["-c", "sleep 30"]),
            Duration::from_millis(200),
        );
        assert!(ran.unwrap().is_none());
        assert!(began.elapsed() < Duration::from_secs(5), "it was stopped, not waited for");
        let said =
            run_within(Command::new("/bin/sh").args(["-c", "echo hi; echo there >&2"]), PATIENCE);
        let said = said.unwrap().expect("it finished");
        assert_eq!(
            (said.stdout.as_slice(), said.stderr.as_slice()),
            (&b"hi\n"[..], &b"there\n"[..])
        );
    }

    #[test]
    fn the_statusline_path_is_one_shell_word_inside_its_json() {
        let plan = plan(harnesses::WITH_ADAPTERS[0], Path::new("/Users/Jo Smith/.muster/extras"));
        assert!(
            plan.then.iter().any(|line| line.contains(
                r#""command": "'/Users/Jo Smith/.muster/extras/claude-code/statusline.sh'""#
            )),
            "{:?}",
            plan.then
        );
    }
}
