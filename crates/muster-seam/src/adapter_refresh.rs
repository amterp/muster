use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread::{self, JoinHandle};

use muster_core::diagnostics::log;
use muster_core::fields;

pub(crate) fn start(commands: &str) {
    let cli = (!commands.is_empty()).then(|| Path::new(commands).join("muster"));
    let _ = start_with(cli, |cli| {
        Command::new(cli)
            .args(["harness", "refresh", "codex", "--login"])
            .stdin(Stdio::null())
            .output()
    });
}

fn start_with(
    cli: Option<PathBuf>,
    run: impl FnOnce(PathBuf) -> std::io::Result<Output> + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    let started = thread::Builder::new().name("adapter-refresh".to_string()).spawn(move || {
        let Some(cli) = cli else {
            log::info(
                "adapter.refresh.skipped",
                fields! {
                    "reason" => "this build staged no CLI; no Codex refresh was attempted",
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
            Ok(output) => failed(&format!(
                "{}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => failed(&error.to_string()),
        }
    });
    if let Err(error) = &started {
        failed(&format!("could not start the refresh worker: {error}"));
    }
    started
}

fn failed(reason: &str) {
    log::warn(
        "adapter.refresh.failed",
        fields! {
            "reason" => reason,
            "impact" => "Codex's copy of the adapter may still be old; muster window says when it differs",
            "fix" => "run muster harness install codex, restart Codex and review changed hooks in /hooks",
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
        let worker = start_with(Some(PathBuf::from("/fake/muster")), move |cli| {
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
