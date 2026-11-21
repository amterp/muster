use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread::{self, JoinHandle};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::harnesses::catalog::{self, AutomaticRefresh};

pub(crate) fn start(commands: &str) {
    let cli = (!commands.is_empty()).then(|| Path::new(commands).join("muster"));
    for (_, adapter) in catalog::adapters() {
        let Some(policy) = adapter.refresh.as_ref().and_then(|refresh| refresh.at_startup) else {
            continue;
        };
        let _ = start_with(cli.clone(), policy, move |cli| {
            Command::new(cli)
                .args(["harness", "refresh", adapter.dir, "--login"])
                .stdin(Stdio::null())
                .output()
        });
    }
}

fn start_with(
    cli: Option<PathBuf>,
    policy: AutomaticRefresh,
    run: impl FnOnce(PathBuf) -> std::io::Result<Output> + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    let started = thread::Builder::new().name("adapter-refresh".to_string()).spawn(move || {
        let Some(cli) = cli else {
            log::info(
                "adapter.refresh.skipped",
                fields! {
                    "reason" => policy.skipped,
                },
            );
            return;
        };
        match run(cli) {
            Ok(output) if output.status.success() => {
                log::info(
                    "adapter.refresh.finished",
                    fields! {
                        "outcome" => String::from_utf8_lossy(&output.stdout).trim(),
                    },
                );
            }
            Ok(output) => failed(
                policy,
                &format!("{}: {}", output.status, String::from_utf8_lossy(&output.stderr).trim()),
            ),
            Err(error) => failed(policy, &error.to_string()),
        }
    });
    if let Err(error) = &started {
        failed(policy, &format!("could not start the refresh worker: {error}"));
    }
    started
}

fn failed(policy: AutomaticRefresh, reason: &str) {
    log::warn(
        "adapter.refresh.failed",
        fields! {
            "reason" => reason,
            "impact" => policy.impact,
            "fix" => policy.repair,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn starting_does_not_wait_for_the_refresh_process() {
        let (began, running) = mpsc::channel();
        let (release, proceed) = mpsc::channel();
        let policy = catalog::named("codex").unwrap().refresh.as_ref().unwrap().at_startup.unwrap();
        let worker = start_with(Some(PathBuf::from("/fake/muster")), policy, move |cli| {
            began.send(cli).unwrap();
            proceed.recv().unwrap();
            Ok(Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout: b"Codex plugin matches this Muster".to_vec(),
                stderr: Vec::new(),
            })
        })
        .unwrap();
        assert_eq!(
            running.recv_timeout(Duration::from_secs(5)).unwrap(),
            Path::new("/fake/muster")
        );
        assert!(!worker.is_finished(), "the launcher returned while the process was running");
        release.send(()).unwrap();
        worker.join().unwrap();
    }
}
