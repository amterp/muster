//! Runs adapter commands and renders their answers.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use muster_adapters::catalog::{self, Adapter, Refresh};
use muster_adapters::install::{InstallContext, PATIENCE, REFRESH_PATIENCE, extras_beside};
use serde_json::json;

use crate::Trouble;
use crate::environment::muster_home;

mod login;

pub fn adapter(name: &str) -> Result<&'static Adapter, String> {
    catalog::named(name).ok_or_else(|| {
        let known: Vec<_> = catalog::adapters().map(|(_, adapter)| adapter.dir).collect();
        format!(
            "Muster ships no adapter for {name:?}, so there is nothing to install. It has one for \
             {}; every other harness is read off its screen (`muster docs harnesses`).",
            known.join(", ")
        )
    })
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
    let beside = executable.as_deref().map(extras_beside);
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

/// Installs the adapter, or with `dry_run` prints what that would run.
pub fn install(
    adapter: &Adapter,
    dry_run: bool,
    json: bool,
    environment: &BTreeMap<String, String>,
    out: &mut impl Write,
    errors: &mut impl Write,
) -> Result<(), Trouble> {
    let extras = extras(environment, std::env::current_exe().ok())?;
    let home = muster_home(environment).map(PathBuf::from);
    let context = InstallContext { extras: &extras, environment, muster_home: home.as_deref() };
    let plan =
        (adapter.install)(&context, dry_run, &mut |words, limit| run(environment, words, limit));
    let printed = |out: &mut dyn Write, ran: bool| {
        let _ = if json {
            writeln!(
                out,
                "{}",
                json!({
                    "harness": adapter.dir,
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
            let _ = writeln!(out, "$ {}", step.line());
        }
        let left = || plan.printed()[done..].join("\n  ");
        let ran = run(environment, command, PATIENCE)
            .map_err(|error| {
                Trouble::Refused(format!(
                    "`{}` could not be run ({error}), so {}'s adapter is not installed. Install \
                     {} or put it on this PATH, then run:\n  {}",
                    command[0],
                    adapter.name,
                    adapter.name,
                    left()
                ))
            })?
            .ok_or_else(|| {
                Trouble::Refused(format!(
                    "`{}` had not finished after {}s and was stopped, so {}'s adapter may be \
                     only partly installed. A harness asking a question on its terminal, or \
                     waiting on the network, holds a command this long; run what is left \
                     yourself, where you can see it:\n  {}",
                    step.line(),
                    PATIENCE.as_secs(),
                    adapter.name,
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
                "`{}` failed ({}), so installing {}'s adapter stopped there; what it said is above.{} Then run what is left:\n  {}",
                step.line(),
                ran.status,
                adapter.name,
                adapter.install_failure,
                left()
            )));
        }
    }
    printed(out, true);
    Ok(())
}

pub fn refresh(
    adapter: &Adapter,
    refresh: &Refresh,
    login: bool,
    environment: &BTreeMap<String, String>,
    out: &mut dyn Write,
) -> Result<(), Trouble> {
    let login_environment;
    let environment = if login {
        login_environment = login::environment(environment).map_err(|reason| {
            Trouble::Refused(format!(
                "{reason}, so {}'s adapter was not refreshed. {}",
                adapter.name, refresh.login_repair
            ))
        })?;
        &login_environment
    } else {
        environment
    };
    let extras = extras(environment, std::env::current_exe().ok())?;
    let home = muster_home(environment).map(PathBuf::from);
    let context = InstallContext { extras: &extras, environment, muster_home: home.as_deref() };
    (refresh.run)(&context, out, &mut |words, limit| run(environment, words, limit))
        .map_err(Trouble::Refused)
}

pub fn stale_copies(environment: &BTreeMap<String, String>) -> Vec<(&'static str, &'static str)> {
    let Ok(extras) = extras(environment, std::env::current_exe().ok()) else { return Vec::new() };
    let home = muster_home(environment).map(PathBuf::from);
    let context = InstallContext { extras: &extras, environment, muster_home: home.as_deref() };
    catalog::adapters()
        .filter_map(|(id, adapter)| {
            let copy = adapter.copy?;
            (copy.differs)(&context).then_some((id, copy.warning))
        })
        .collect()
}

fn run(
    environment: &BTreeMap<String, String>,
    words: &[String],
    limit: Duration,
) -> io::Result<Option<Output>> {
    let mut command = Command::new(&words[0]);
    command.args(&words[1..]).env_clear().envs(environment);
    run_within(&mut command, limit)
}

/// Runs `command` to its end and says what it printed, or stops it and says nothing once it has
/// run for `limit`.
fn run_within(command: &mut Command, limit: Duration) -> io::Result<Option<Output>> {
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
}
