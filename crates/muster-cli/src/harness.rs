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
    pub commands: Vec<Step>,
    pub then: Vec<String>,
}

/// One of a harness's own commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub words: Vec<String>,
    /// A failure that still leaves the command's work done: what the harness says on stderr, and
    /// what that means for the person.
    pub done_if_said: Option<(&'static str, &'static str)>,
}

impl Step {
    fn of(words: &[&str]) -> Step {
        Step { words: words.iter().map(ToString::to_string).collect(), done_if_said: None }
    }
}

impl Plan {
    fn printed(&self) -> Vec<String> {
        self.commands.iter().map(|command| line(&command.words)).collect()
    }
}

/// Where Codex has a `muster` marketplace registered, by the path it resolved when it was added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered(pub PathBuf);

/// The root of the `muster` marketplace in what `codex plugin marketplace list --json` printed,
/// or None when there is none, or the listing is not one this knows how to read.
pub fn registered_in(listing: &str) -> Option<Registered> {
    let listing: serde_json::Value = serde_json::from_str(listing).ok()?;
    let muster =
        listing["marketplaces"].as_array()?.iter().find(|entry| entry["name"] == "muster")?;
    Some(Registered(PathBuf::from(muster["root"].as_str()?)))
}

/// Whether a `muster` marketplace Codex has registered is one an earlier Muster install left,
/// which installing from `extras` should replace: a different path from `extras` that has gone, or
/// that is where Muster keeps a version's adapters - under `<muster home>/daemon/`, or inside an
/// app bundle. Codex records a marketplace by its resolved path, so `~/.muster/extras` registers
/// as the version it pointed at, and once Muster is updated Codex refuses to add the new one
/// beside it. A marketplace added from anywhere else, such as a checkout of Muster's source, is
/// somebody's choice and is not this install's to replace.
pub fn stale_registration(
    registered: &Registered,
    extras: &Path,
    muster_home: Option<&Path>,
) -> bool {
    let root = &registered.0;
    let ours = extras.canonicalize().unwrap_or_else(|_| extras.to_path_buf());
    if *root == ours || root == extras {
        return false;
    }
    let versions = muster_home.map(|home| home.join("daemon"));
    let under_versions = versions.is_some_and(|versions| {
        root.starts_with(&versions)
            || versions.canonicalize().is_ok_and(|real| root.starts_with(real))
    });
    !root.exists() || under_versions || root.ends_with("Contents/Resources/extras")
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

/// What installing `harness`'s adapter from `extras` takes. `replace` says the `muster`
/// marketplace Codex has registered is a stale one to remove first ([`stale_registration`]).
pub fn plan(harness: Harness, extras: &Path, replace: bool) -> Plan {
    let readme = shell_word(&extras.join(harness.dir).join("README.md").display().to_string());
    let extras_path = extras.display().to_string();
    match harness.kind {
        Kind::ClaudeCode => Plan {
            commands: vec![
                Step::of(&["claude", "plugin", "marketplace", "add", &extras_path]),
                Step::of(&["claude", "plugin", "install", "muster@muster"]),
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
            commands: replace
                .then(|| Step::of(&["codex", "plugin", "marketplace", "remove", "muster"]))
                .into_iter()
                .chain([
                    Step::of(&["codex", "plugin", "marketplace", "add", &extras_path]),
                    Step::of(&["codex", "plugin", "add", "muster-codex@muster"]),
                    // Last, so that a Codex too old to know the flag has its plugin already.
                    Step {
                        done_if_said: Some((
                            "Unknown feature flag",
                            "This Codex has no daemon_auto_start, and so no shared app-server to \
                             turn off.",
                        )),
                        ..Step::of(&["codex", "features", "disable", "daemon_auto_start"])
                    },
                ])
                .collect(),
            then: vec![
                "Codex runs a hook only once you trust it: review them in /hooks in a session."
                    .to_string(),
                "Sessions already running take the plugin up when they restart.".to_string(),
                "daemon_auto_start is now off, so each Codex session runs its own commands and \
                 hooks. Left on, Codex runs every session's in one shared app-server that carries \
                 the first session's pane, and Muster refuses the reports that come through it. \
                 `codex agents` needs that server; `codex features enable daemon_auto_start` \
                 turns it back on."
                    .to_string(),
                "An app-server already running still takes new sessions until `codex app-server \
                 daemon stop`, which ends the sessions running in it."
                    .to_string(),
                "Codex keeps its own copy of the plugin, so run this again after updating Muster."
                    .to_string(),
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
    let replace = harness.kind == Kind::Codex
        && codex_registration(environment).is_some_and(|registered| {
            let home = muster_home(environment).map(PathBuf::from);
            stale_registration(&registered, &extras, home.as_deref())
        });
    let plan = plan(harness, &extras, replace);
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
    for (done, step) in plan.commands.iter().enumerate() {
        let command = &step.words;
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
        let said = String::from_utf8_lossy(&ran.stderr);
        if let Some((_, meaning)) =
            step.done_if_said.filter(|(text, _)| !ran.status.success() && said.contains(text))
        {
            let _ = if json { writeln!(errors, "{meaning}") } else { writeln!(out, "{meaning}") };
            continue;
        }
        let _ = errors.write_all(&ran.stderr);
        if !ran.status.success() {
            return Err(Trouble::Refused(format!(
                "`{}` failed ({}), so installing {}'s adapter stopped there; what it said is above. If it \
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

/// Where Codex has a `muster` marketplace registered, by asking it. None when it has none, or
/// cannot be asked, or answers in a way this cannot read: an older Codex has no `--json` there.
fn codex_registration(environment: &BTreeMap<String, String>) -> Option<Registered> {
    let mut listing = Command::new("codex");
    listing.args(["plugin", "marketplace", "list", "--json"]).env_clear().envs(environment);
    let listed = run_within(&mut listing, PATIENCE).ok()??;
    registered_in(&String::from_utf8_lossy(&listed.stdout))
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

    /// What `codex plugin marketplace list --json` printed for Codex 0.160.0, a stale install's
    /// `muster` among its marketplaces (`corpus/codex-0.160.0/marketplace.txt`).
    #[test]
    fn the_muster_marketplace_is_read_out_of_codexs_listing() {
        let listing = r#"{"marketplaces": [
            {"name": "openai-api-curated", "root": "/home/me/.codex/.tmp/plugins"},
            {"name": "muster", "root": "/home/me/.muster/daemon/0.14.0/extras",
             "marketplaceSource": {"sourceType": "local",
                                   "source": "/home/me/.muster/daemon/0.14.0/extras"}}]}"#;
        assert_eq!(
            registered_in(listing),
            Some(Registered(PathBuf::from("/home/me/.muster/daemon/0.14.0/extras")))
        );
        assert_eq!(registered_in(r#"{"marketplaces": []}"#), None);
        assert_eq!(registered_in("MARKETPLACE  ROOT\nmuster  /somewhere"), None, "not JSON");
    }

    #[test]
    fn only_a_muster_installs_own_marketplace_is_replaced() {
        let root = std::env::temp_dir().join(format!("muster-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join(".muster");
        let (old, new) = (home.join("daemon/0.14.0/extras"), home.join("daemon/0.15.0/extras"));
        let checkout = root.join("src/muster/extras");
        for directory in [&old, &new, &checkout] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let linked = home.join("extras");
        std::os::unix::fs::symlink(&new, &linked).unwrap();
        let new = new.canonicalize().unwrap();
        let stale = |registered: &Path| {
            stale_registration(&Registered(registered.to_path_buf()), &linked, Some(&home))
        };

        assert!(stale(&old.canonicalize().unwrap()), "an earlier version's adapters");
        assert!(!stale(&new), "the version the link points at is what installing adds again");
        assert!(stale(&root.join("gone/extras")), "a path that has gone");
        assert!(stale(Path::new("/Volumes/Old/Muster.app/Contents/Resources/extras")));
        assert!(!stale(&checkout.canonicalize().unwrap()), "somebody's checkout");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_statusline_path_is_one_shell_word_inside_its_json() {
        let plan =
            plan(harnesses::WITH_ADAPTERS[0], Path::new("/Users/Jo Smith/.muster/extras"), false);
        assert!(
            plan.then.iter().any(|line| line.contains(
                r#""command": "'/Users/Jo Smith/.muster/extras/claude-code/statusline.sh'""#
            )),
            "{:?}",
            plan.then
        );
    }
}
