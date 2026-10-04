//! A wake handed to a session by its harness's own command, typing nothing into its pane: the
//! command the agent's manifest names in `[session] wake`, given the session's id its hooks
//! reported (MIP-5, section 7). Codex's is `codex queue`, which starts a turn in the running
//! session as if the message were typed there, and leaves a draft in its composer alone.
//!
//! The doorbell chooses it over typing whenever it would ring the agent idle, and only then: at
//! an approval prompt Codex stores a queued message and never submits it, and at work it holds
//! one until the turn ends, which an urgent ring typed into the running turn does better
//! (`docs/observations/codex-0.154.0.md`, section 9).

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::pty;

/// How long the command may take. It runs on the doorbell's thread, which rings nobody else
/// meanwhile; `codex queue` answers in well under a second.
const PATIENCE: Duration = Duration::from_secs(5);

/// How much of what a failed command printed is kept, for the log.
const SAID: usize = 4096;

/// Runs `arguments` as a login shell of the user's would, so it finds the harness a pane finds:
/// a daemon started by launchd has a PATH without Homebrew's directory in it. The arguments are
/// handed to the shell as arguments, never as script, so the message is never read as one.
pub(crate) fn run(arguments: &[String]) -> Result<(), Failed> {
    let environment: Vec<_> = std::env::vars_os().collect();
    let shell = pty::default_shell(&environment);
    let script =
        if shell.rsplit('/').next() == Some("fish") { "exec $argv" } else { "exec \"$0\" \"$@\"" };
    let mut child = Command::new(&shell)
        .args(["-l", "-c", script])
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Failed::Refused(format!("could not start {shell}: {error}")))?;
    // Read as it is written: a command saying more than a pipe holds would otherwise wait for us
    // to read it, and be killed as stalled. The start is kept for the log, the rest dropped.
    let stderr = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut chunk = [0; 4096];
            while let Ok(read) = stderr.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                let room = SAID.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..read.min(room)]);
            }
            String::from_utf8_lossy(&kept).into_owned()
        })
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) =
            child.try_wait().map_err(|error| Failed::Refused(error.to_string()))?
        {
            break status;
        }
        if started.elapsed() > PATIENCE {
            drop(child.kill());
            drop(child.wait());
            return Err(Failed::Stalled(format!(
                "it was still running after {}s",
                PATIENCE.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if status.success() {
        return Ok(());
    }
    let said = stderr.and_then(|reading| reading.join().ok()).unwrap_or_default();
    let said = said.trim();
    let said = said.char_indices().nth(300).map_or(said, |(at, _)| &said[..at]);
    Err(Failed::Refused(format!("it exited with {status}: {said}")))
}

/// Why a command did not hand its message over.
#[derive(Debug)]
pub(crate) enum Failed {
    /// It could not start, or said no: a session gone, a harness not on the PATH. It will say the
    /// same next time.
    Refused(String),
    /// It took too long, which a busy machine can make it do once; it may still have handed the
    /// message over.
    Stalled(String),
}

impl Failed {
    pub(crate) fn why(&self) -> &str {
        match self {
            Failed::Refused(why) | Failed::Stalled(why) => why,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arguments_reach_the_program_whole_and_a_failure_says_what_it_printed() {
        let scratch = std::env::temp_dir().join(format!("muster-command-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let out = scratch.join("argv");
        let program = scratch.join("program");
        std::fs::write(
            &program,
            format!("#!/bin/sh\nprintf '[%s]' \"$@\" > '{}'\n", out.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let message = "say \"hi\"; $(echo injected) `echo injected` *";
        let arguments = [program.display().to_string(), "--message".into(), message.into()];
        run(&arguments).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), format!("[--message][{message}]"));

        let failing = ["/bin/sh".to_string(), "-c".into(), "echo nobody there >&2; exit 1".into()];
        let failed = run(&failing).unwrap_err();
        assert!(matches!(failed, Failed::Refused(_)), "{failed:?}");
        assert!(failed.why().contains("nobody there"), "{failed:?}");

        // More than a pipe holds, then a failure: refused at once, not killed as stalled.
        let chatty = [
            "/bin/sh".to_string(),
            "-c".into(),
            "head -c 200000 /dev/zero | tr '\\0' x >&2; exit 1".into(),
        ];
        let failed = run(&chatty).unwrap_err();
        assert!(matches!(failed, Failed::Refused(_)), "{failed:?}");
        drop(std::fs::remove_dir_all(&scratch));
    }
}
