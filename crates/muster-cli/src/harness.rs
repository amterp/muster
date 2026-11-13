//! `muster harness install`: a harness's adapter, installed through the harness's own command.
//!
//! Never by writing into a harness's configuration: a person's harness config may be managed by
//! a tool of their own that rewrites it, and the harness's command is the one way in that both
//! agree on. What no command of the harness's can do is printed for the person to do by hand.

use std::collections::BTreeMap;
use std::io::Read;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use muster_core::harnesses::{self, Harness, Kind};
use serde_json::json;

use crate::Trouble;
use crate::environment::muster_home;
use crate::render::shell_word;

mod login;
pub use login::environment as login_environment;

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
            commands: codex_plugin_commands(extras, replace)
                .into_iter()
                .chain([
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
                "Muster refreshes Codex's copy in the background at launch when its content \
                 changes. Review changed hooks in /hooks after restarting Codex."
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

/// Where Codex keeps what it installs: `$CODEX_HOME`, else `~/.codex`.
pub fn codex_home(environment: &BTreeMap<String, String>) -> Option<PathBuf> {
    if let Some(explicit) = environment.get("CODEX_HOME").filter(|home| !home.is_empty()) {
        return Some(PathBuf::from(explicit));
    }
    let home = environment.get("HOME").filter(|home| !home.is_empty())?;
    Some(Path::new(home).join(".codex"))
}

/// Whether the copy of Muster's Codex plugin that Codex installed differs from the one in
/// `extras`. False when Codex has no copy: nothing is running an older adapter then, and there is
/// nothing to tell the person to refresh.
///
/// Codex copies the plugin into `<codex home>/plugins/cache/muster/muster-codex/<version>/` when
/// it is installed and never again, so the copy keeps whatever Muster it came from. Content is
/// compared rather than a version, because development builds share one. Markdown is left out:
/// it is documentation, and Codex asks for hooks to be trusted again only when they change, so
/// telling somebody to reinstall over a README would cost them a review for nothing.
pub fn codex_copy_differs(extras: &Path, codex_home: &Path) -> bool {
    let Some(copy) = newest_subdirectory(&codex_home.join("plugins/cache/muster/muster-codex"))
    else {
        return false;
    };
    let plugin = extras.join("codex");
    let mut files = Vec::new();
    collect_files(&plugin, Path::new(""), &mut files);
    files.iter().filter(|relative| relative.extension().is_none_or(|ext| ext != "md")).any(
        |relative| {
            std::fs::read(plugin.join(relative)).ok() != std::fs::read(copy.join(relative)).ok()
        },
    )
}

/// [`codex_copy_differs`] for this machine: the Codex under this environment's home, against the
/// adapters of the Muster this CLI belongs to. False when either cannot be found.
pub fn codex_adapter_copy_differs(environment: &BTreeMap<String, String>) -> bool {
    let Some(home) = codex_home(environment) else { return false };
    extras(environment, std::env::current_exe().ok())
        .is_ok_and(|extras| codex_copy_differs(&extras, &home))
}

/// The directory in `directory` changed most recently: Codex names a local plugin's version
/// itself, so which one it is is not Muster's to say.
fn newest_subdirectory(directory: &Path) -> Option<PathBuf> {
    std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max()
        .map(|(_, path)| path)
}

/// Every file under `root`, as paths relative to it.
fn collect_files(root: &Path, relative: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root.join(relative)) else { return };
    for entry in entries.filter_map(Result::ok) {
        let path = relative.join(entry.file_name());
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => collect_files(root, &path, found),
            Ok(_) => found.push(path),
            Err(_) => {}
        }
    }
}

fn installation_plan(
    harness: Harness,
    extras: &Path,
    dry_run: bool,
    environment: &BTreeMap<String, String>,
) -> Plan {
    let replace = !dry_run
        && harness.kind == Kind::Codex
        && codex_registration(environment).is_some_and(|registered| {
            let home = muster_home(environment).map(PathBuf::from);
            stale_registration(&registered, extras, home.as_deref())
        });
    let mut plan = plan(harness, extras, replace);
    if dry_run && harness.kind == Kind::Codex {
        plan.then.insert(
            0,
            "No Codex command was run to check its marketplace. An install removes an earlier \
             Muster's marketplace first when needed; if running this plan reports a different \
             source, run `codex plugin marketplace remove muster` before adding it again."
                .to_string(),
        );
    }
    plan
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
    let plan = installation_plan(harness, &extras, dry_run, environment);
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
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            let _ = send.send(bytes);
        });
        receive
    };
    let stdout = drain(child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let began = Instant::now();
    let mut status = None;
    let mut output = None;
    let mut errors = None;
    loop {
        if status.is_none() {
            status = child.try_wait()?;
        }
        if output.is_none() {
            output = stdout.try_recv().ok();
        }
        if errors.is_none() {
            errors = stderr.try_recv().ok();
        }
        if let (Some(status), Some(stdout), Some(stderr)) = (status, &output, &errors) {
            return Ok(Some(Output { status, stdout: stdout.clone(), stderr: stderr.clone() }));
        }
        if began.elapsed() >= limit {
            // A harness can leave a child holding its pipes even after the harness itself exits.
            // Stop the group and never wait for pipe-reader threads past the deadline.
            if let Ok(group) = i32::try_from(child.id()) {
                // SAFETY: the child created its own process group; this signals only that group.
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn line(command: &[String]) -> String {
    command.iter().map(|word| shell_word(word)).collect::<Vec<_>>().join(" ")
}

const REFRESH_PATIENCE: Duration = Duration::from_secs(10);

/// Refreshes a plugin Codex already installed, leaving first installs and feature settings alone.
pub fn refresh(
    environment: &BTreeMap<String, String>,
    out: &mut impl Write,
) -> Result<(), Trouble> {
    let extras = extras(environment, std::env::current_exe().ok())?;
    let Some(home) = codex_home(environment) else {
        let _ = writeln!(out, "Codex plugin not installed: no Codex home.");
        return Ok(());
    };
    refresh_with(&extras, &home, environment, out, |words| {
        let mut command = Command::new(&words[0]);
        command.args(&words[1..]).env_clear().envs(environment);
        run_within(&mut command, REFRESH_PATIENCE)
    })
}

fn refresh_with(
    extras: &Path,
    home: &Path,
    environment: &BTreeMap<String, String>,
    out: &mut impl Write,
    mut run: impl FnMut(&[String]) -> std::io::Result<Option<Output>>,
) -> Result<(), Trouble> {
    if newest_subdirectory(&home.join("plugins/cache/muster/muster-codex")).is_none() {
        let _ = writeln!(out, "Codex plugin not installed; nothing to refresh.");
        return Ok(());
    }
    if !codex_copy_differs(extras, home) {
        let _ = writeln!(out, "Codex plugin matches this Muster; hooks were left alone.");
        return Ok(());
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join("plugins/.muster-refresh.lock"))
        .map_err(|error| {
            Trouble::Refused(format!("could not lock Codex's adapter refresh: {error}"))
        })?;
    // SAFETY: lock owns an open file descriptor for the lifetime of this refresh.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            let _ = writeln!(out, "Another Muster is already refreshing Codex's plugin.");
            return Ok(());
        }
        return Err(Trouble::Refused(format!("could not lock Codex's adapter refresh: {error}")));
    }
    if !codex_copy_differs(extras, home) {
        let _ = writeln!(out, "Codex plugin matches this Muster; another launch refreshed it.");
        return Ok(());
    }
    let listing = Step::of(&["codex", "plugin", "marketplace", "list", "--json"]);
    let listed = refresh_step(&listing, &mut run)?;
    let value: serde_json::Value = serde_json::from_slice(&listed.stdout)
        .map_err(|error| refresh_failed(&listing, &format!("its listing was not JSON: {error}")))?;
    if !value["marketplaces"].is_array() {
        return Err(refresh_failed(&listing, "its listing had no marketplaces array"));
    }
    let Some(registered) = registered_in(&String::from_utf8_lossy(&listed.stdout)) else {
        let _ =
            writeln!(out, "Codex's muster marketplace is not registered; nothing was installed.");
        return Ok(());
    };
    let muster_home = muster_home(environment).map(PathBuf::from);
    let replace = stale_registration(&registered, extras, muster_home.as_deref());
    let ours = extras.canonicalize().unwrap_or_else(|_| extras.to_path_buf());
    let same_source = registered.0 == ours || registered.0 == extras;
    let Some(commands) = refresh_commands(replace, same_source, extras) else {
        let _ = writeln!(out, "Codex's muster marketplace uses another source; it was left alone.");
        return Ok(());
    };
    for command in commands {
        refresh_step(&command, &mut run)?;
    }
    if codex_copy_differs(extras, home) {
        return Err(Trouble::Refused(
            "Codex reported success but its plugin copy still differs from this Muster. Run \
             `muster harness install codex` and review the hooks in `/hooks`."
                .to_string(),
        ));
    }
    let _ =
        writeln!(out, "Codex plugin refreshed; restart Codex and review changed hooks in /hooks.");
    Ok(())
}

fn refresh_commands(replace: bool, same_source: bool, extras: &Path) -> Option<Vec<Step>> {
    if !replace && !same_source {
        return None;
    }
    Some(codex_plugin_commands(extras, replace))
}

fn codex_plugin_commands(extras: &Path, replace: bool) -> Vec<Step> {
    let mut commands = Vec::new();
    if replace {
        commands.push(Step::of(&["codex", "plugin", "marketplace", "remove", "muster"]));
    }
    commands.push(Step::of(&[
        "codex",
        "plugin",
        "marketplace",
        "add",
        &extras.display().to_string(),
    ]));
    commands.push(Step::of(&["codex", "plugin", "add", "muster-codex@muster"]));
    commands
}

fn refresh_step(
    step: &Step,
    run: &mut impl FnMut(&[String]) -> std::io::Result<Option<Output>>,
) -> Result<Output, Trouble> {
    let ran = run(&step.words)
        .map_err(|error| refresh_failed(step, &error.to_string()))?
        .ok_or_else(|| refresh_failed(step, "it timed out after 10 seconds and was stopped"))?;
    if !ran.status.success() {
        return Err(refresh_failed(
            step,
            &format!("{}: {}", ran.status, String::from_utf8_lossy(&ran.stderr).trim()),
        ));
    }
    Ok(ran)
}

fn refresh_failed(step: &Step, reason: &str) -> Trouble {
    Trouble::Refused(format!(
        "`{}` failed ({reason}), so Codex's adapter was not refreshed. Its existing copy may \
         still be old; `muster window` says when it differs. Run `muster harness install codex` \
         and review changed hooks in `/hooks`.",
        line(&step.words)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_does_not_wait_for_a_child_holding_the_output_pipes() {
        let began = Instant::now();
        let ran = run_within(
            Command::new("/bin/sh").args(["-c", "/bin/sleep 10 & wait"]),
            Duration::from_millis(100),
        );
        assert!(ran.unwrap().is_none());
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "the child's pipes held the timeout open"
        );
    }

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

    /// Muster's adapters with a Codex plugin in them, and a Codex home holding a copy of it.
    struct Copies(PathBuf);

    impl Copies {
        const FILES: [(&'static str, &'static str); 4] = [
            (".codex-plugin/plugin.json", "{\"name\": \"muster-codex\"}"),
            ("hooks/hooks.json", "{\"hooks\": 1}"),
            ("messaging-hooks.json", "{}"),
            ("README.md", "# the plugin"),
        ];

        fn new(test: &str) -> Copies {
            let root =
                std::env::temp_dir().join(format!("muster-copy-{}-{test}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let copies = Copies(root);
            for (name, text) in Self::FILES {
                Copies::write(&copies.extras().join("codex"), name, text);
            }
            copies
        }

        fn extras(&self) -> PathBuf {
            self.0.join("extras")
        }

        fn codex_home(&self) -> PathBuf {
            self.0.join("codex-home")
        }

        fn version(&self, version: &str) -> PathBuf {
            self.codex_home().join("plugins/cache/muster/muster-codex").join(version)
        }

        fn write(directory: &Path, name: &str, text: &str) {
            let path = directory.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        /// What Codex copies in when the plugin is installed: every file, as shipped.
        fn install(&self, version: &str) {
            for (name, text) in Self::FILES {
                Self::write(&self.version(version), name, text);
            }
        }

        fn differs(&self) -> bool {
            codex_copy_differs(&self.extras(), &self.codex_home())
        }
    }

    impl Drop for Copies {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_codex_copy_that_matches_the_shipped_plugin_says_nothing() {
        let copies = Copies::new("matches");
        copies.install("local");
        assert!(!copies.differs());
    }

    #[test]
    fn a_codex_copy_with_other_hooks_differs() {
        let copies = Copies::new("hooks");
        copies.install("local");
        Copies::write(&copies.version("local"), "hooks/hooks.json", "{\"hooks\": 0}");
        assert!(copies.differs());
    }

    #[test]
    fn a_codex_copy_missing_a_file_the_plugin_ships_differs() {
        let copies = Copies::new("missing");
        copies.install("local");
        std::fs::remove_file(copies.version("local").join("messaging-hooks.json")).unwrap();
        assert!(copies.differs());
    }

    #[test]
    fn a_documentation_change_alone_is_not_a_reason_to_reinstall() {
        let copies = Copies::new("readme");
        copies.install("local");
        Copies::write(&copies.version("local"), "README.md", "# an older readme");
        assert!(!copies.differs());
    }

    #[test]
    fn without_a_codex_copy_there_is_nothing_to_refresh() {
        let copies = Copies::new("none");
        assert!(!copies.differs(), "no Codex home at all");
        std::fs::create_dir_all(copies.codex_home().join("plugins/cache/muster")).unwrap();
        assert!(!copies.differs(), "a marketplace Codex never installed the plugin from");
    }

    #[test]
    fn the_newest_codex_copy_is_the_one_compared() {
        let copies = Copies::new("newest");
        copies.install("0.1");
        Copies::write(&copies.version("0.1"), "hooks/hooks.json", "{\"hooks\": 0}");
        std::thread::sleep(Duration::from_millis(30));
        copies.install("0.2");
        assert!(!copies.differs(), "the older copy is not what Codex runs");
    }

    #[test]
    fn codex_keeps_its_plugins_where_it_was_told_to_or_in_the_home_directory() {
        let environment = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
        };
        assert_eq!(
            codex_home(&environment(&[("HOME", "/home/me"), ("CODEX_HOME", "/elsewhere")])),
            Some(PathBuf::from("/elsewhere"))
        );
        assert_eq!(
            codex_home(&environment(&[("HOME", "/home/me")])),
            Some(PathBuf::from("/home/me/.codex"))
        );
        assert_eq!(codex_home(&environment(&[])), None);
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

    #[test]
    fn a_refresh_timeout_names_the_command_and_leaves_the_copy_unchanged() {
        let copies = Copies::new("refresh-timeout");
        copies.install("local");
        Copies::write(&copies.version("local"), "hooks/hooks.json", "{}");
        let error = refresh_with(
            &copies.extras(),
            &copies.codex_home(),
            &BTreeMap::new(),
            &mut Vec::new(),
            |words| {
                assert_eq!(words, ["codex", "plugin", "marketplace", "list", "--json"]);
                Ok(None)
            },
        );
        let Err(Trouble::Refused(reason)) = error else { panic!("{error:?}") };
        assert!(reason.contains("codex plugin marketplace list --json"), "{reason}");
        assert!(reason.contains("timed out after 10 seconds"), "{reason}");
        assert!(copies.differs());
    }
}
