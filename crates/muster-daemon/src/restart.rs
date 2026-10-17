//! Restarting a pane's agent where it runs: its input emptied, its program ended, and its
//! session's resume typed at the shell it leaves behind.
//!
//! The order is what makes it safe to do to a program that has stopped reading. Such a program
//! leaves input waiting in its terminal and in the daemon's queue behind it, wheel reports by the
//! kilobyte (a_2is4QBDSZ), and its shell reads whatever is left as typed text the moment it
//! exits, so the input is emptied before the program is ended, and again before the resume is
//! typed. Input typed meanwhile is refused rather than queued, since it would reach the shell
//! ahead of the resume.

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;

use crate::pane::PaneIo;
use crate::pty;
use muster_daemon_proto as proto;
use proto::answer::Detail;

use crate::session::Reply;
use crate::writer::Input;

/// How long a program asked to end has before it is killed. Claude Code and Codex end within a
/// second of SIGTERM; this leaves room for one saving its session on a loaded machine.
const GRACE: Duration = Duration::from_secs(5);

/// A debug build's [`GRACE`] in milliseconds, when set, so a test need not wait for it.
const GRACE_SAID: &str = "MUSTER_DAEMON_RESTART_GRACE_MS";

/// How long a killed program's processes have to be gone.
const KILLED_WITHIN: Duration = Duration::from_secs(2);

/// How long the shell has, once the program is gone, to take the terminal back and draw its
/// prompt. Past it the resume is typed anyway, and the shell reads it once it is ready.
const PROMPT_WITHIN: Duration = Duration::from_secs(5);

/// How long a shell that says nothing of its prompt has to be quiet for to be taken as at it.
const QUIET: Duration = Duration::from_millis(300);

const LOOK_EVERY: Duration = Duration::from_millis(50);

/// What a restart waits for, apart so a test can make it short.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    pub(crate) grace: Duration,
    pub(crate) killed_within: Duration,
    pub(crate) prompt_within: Duration,
    pub(crate) quiet: Duration,
    pub(crate) look_every: Duration,
}

impl Timing {
    pub(crate) fn daemon() -> Timing {
        let grace = if cfg!(debug_assertions)
            && let Ok(said) = std::env::var(GRACE_SAID)
            && let Ok(millis) = said.trim().parse::<u64>()
        {
            Duration::from_millis(millis)
        } else {
            GRACE
        };
        Timing {
            grace,
            killed_within: KILLED_WITHIN,
            prompt_within: PROMPT_WITHIN,
            quiet: QUIET,
            look_every: LOOK_EVERY,
        }
    }
}

/// Why `name=value` cannot be set for a resumed agent, if it cannot.
pub(crate) fn variable_fault(name: &str, value: &str) -> Option<String> {
    let mut letters = name.chars();
    let named = letters.next().is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && letters.all(|letter| letter == '_' || letter.is_ascii_alphanumeric());
    if !named {
        return Some(format!(
            "`{name}` is not a variable's name: a letter or `_`, then letters, digits and `_`"
        ));
    }
    if crate::spawn::is_reserved(name) {
        return Some(format!(
            "{name} is set by Muster on every pane, so a restart does not change it"
        ));
    }
    if value.chars().any(char::is_control) {
        return Some(format!(
            "{name}'s value is typed at the pane's shell as part of one line, so it cannot hold \
             a newline or any other control character"
        ));
    }
    None
}

/// What a restart does to a pane, apart so its order is a unit test.
pub(crate) trait Target {
    /// Refuses typed input from now, or takes it again.
    fn refuse_input(&self, refused: bool);
    /// Empties what the program has yet to read, in the daemon and in the terminal.
    fn flush_input(&self) -> io::Result<()>;
    fn terminate(&self);
    fn kill(&self);
    /// Whether anything of the agent's process group is left.
    fn running(&self) -> bool;
    /// Whether the pane's shell holds its terminal.
    fn shell_holds(&self) -> bool;
    /// Whether detection has seen the agent leave, so the pane's record no longer names it.
    fn agent_left(&self) -> bool;
    /// How many times the pane has said which directory it is in, which a shell does at each
    /// prompt.
    fn directories_said(&self) -> u64;
    fn drawn_at(&self) -> Option<Instant>;
    /// Queues `line` and a Return past the refusal. False when it could not be queued.
    fn type_line(&self, line: &str) -> bool;
}

/// A checked restart, to run with the session unlocked.
pub(crate) struct Restarting {
    pub(crate) pane: String,
    pub(crate) program: String,
    pub(crate) session: String,
    pub(crate) line: String,
    pub(crate) target: Live,
}

/// Without its line, which can hold a variable's value.
impl std::fmt::Debug for Restarting {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Restarting")
            .field("pane", &self.pane)
            .field("program", &self.program)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

/// The pane a restart works on, as the daemon holds it.
pub(crate) struct Live {
    pub(crate) io: Arc<PaneIo>,
    pub(crate) group: i32,
    pub(crate) shell: i32,
}

impl Target for Live {
    fn refuse_input(&self, refused: bool) {
        self.io.discards().set_restarting(refused);
    }

    fn flush_input(&self) -> io::Result<()> {
        self.io.flush_input()
    }

    fn terminate(&self) {
        pty::terminate_group(self.group);
    }

    fn kill(&self) {
        pty::kill_group(self.group);
    }

    fn running(&self) -> bool {
        pty::group_exists(self.group)
    }

    fn shell_holds(&self) -> bool {
        self.io.foreground_group() == Some(self.shell)
    }

    fn agent_left(&self) -> bool {
        self.io.agent_group() != u32::try_from(self.group).ok()
    }

    fn directories_said(&self) -> u64 {
        self.io.directories_said()
    }

    fn drawn_at(&self) -> Option<Instant> {
        self.io.drawn_at()
    }

    fn type_line(&self, line: &str) -> bool {
        self.io.queue_restart_line(Input::Chore { text: line.to_string(), enter: true })
    }
}

/// How the program ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ended {
    Asked,
    Killed,
}

/// Why a restart stopped short, and where it left the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failed {
    /// Nothing was ended.
    NotFlushed(String),
    /// The program is still running after SIGKILL.
    StillRunning,
    /// The program ended and the shell never took its terminal back.
    ShellAway,
    /// The program ended and the resume could not be typed.
    NotTyped(String),
}

impl Restarting {
    pub(crate) fn run(self) -> Reply {
        let Restarting { pane, program, session, line, target } = self;
        let outcome = restart(&target, &line, Timing::daemon());
        said(&pane, &program, &session, &line, &outcome)
    }
}

/// Restarts the agent `target` holds, typing `line` at its shell once it has gone.
pub(crate) fn restart(target: &impl Target, line: &str, timing: Timing) -> Result<Ended, Failed> {
    target.refuse_input(true);
    let outcome = restart_refusing(target, line, timing);
    target.refuse_input(false);
    outcome
}

fn restart_refusing(target: &impl Target, line: &str, timing: Timing) -> Result<Ended, Failed> {
    target.flush_input().map_err(|error| Failed::NotFlushed(error.to_string()))?;
    let prompts = target.directories_said();
    target.terminate();
    let ended = if until(timing.grace, timing.look_every, || !target.running()) {
        Ended::Asked
    } else {
        target.kill();
        if !until(timing.killed_within, timing.look_every, || !target.running()) {
            return Err(Failed::StillRunning);
        }
        Ended::Killed
    };
    let gone = Instant::now();
    let deadline = gone + timing.prompt_within;
    if !until(timing.prompt_within, timing.look_every, || target.shell_holds()) {
        return Err(Failed::ShellAway);
    }
    // Detection seeing the agent go is waited for too, so that once this answers, the pane's
    // record names no agent until the resumed one starts, and a wait for it to be idle is not
    // met by the one that was ended.
    let at_prompt = || {
        target.agent_left()
            && (target.directories_said() > prompts
                || target.drawn_at().is_none_or(|drawn| drawn.elapsed() >= timing.quiet))
    };
    until(deadline.saturating_duration_since(Instant::now()), timing.look_every, at_prompt);
    target.flush_input().map_err(|error| Failed::NotTyped(error.to_string()))?;
    if !target.type_line(line) {
        return Err(Failed::NotTyped("the pane's input queue is full or gone".to_string()));
    }
    Ok(ended)
}

/// Whether `done` came true within `within`, looking every `every`.
fn until(within: Duration, every: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(every);
    }
}

/// The answer to a restart, logged as it is given. The line is in the answer, which goes to
/// whoever asked, and never in the log: an environment change can carry a proxy's password.
fn said(
    pane: &str,
    program: &str,
    session: &str,
    line: &str,
    outcome: &Result<Ended, Failed>,
) -> Reply {
    match outcome {
        Ok(ended) => {
            log::info(
                "daemon.pane.restarted",
                fields! {
                    "pane" => pane,
                    "program" => program,
                    "session" => session,
                    "killed" => *ended == Ended::Killed,
                },
            );
            let restarted = proto::Restarted {
                program: program.to_string(),
                killed: *ended == Ended::Killed,
                typed: line.to_string(),
            };
            Reply { detail: Some(Box::new(Detail::Restarted(restarted))), ..Reply::done() }
        }
        Err(failed) => {
            let (what, impact, check) = match failed {
                Failed::NotFlushed(error) => (
                    format!(
                        "could not empty the pane's input ({error}), so {program} was left running"
                    ),
                    "nothing was ended, and the agent runs on as it was",
                    "whether the pane's terminal still exists; restarting again tries again",
                ),
                Failed::StillRunning => (
                    format!("{program} was still running after SIGKILL"),
                    "the agent's processes are still in the pane, and its session was not resumed",
                    "whether the program is stuck in the kernel, as on a hung mount; it may end \
                     on its own once that answers",
                ),
                Failed::ShellAway => (
                    format!("{program} ended, and the pane's shell did not take its terminal back"),
                    "the pane is left with whatever holds its terminal, and the session was not \
                     resumed",
                    "what is running in the pane; the session can be resumed by hand",
                ),
                Failed::NotTyped(error) => (
                    format!("{program} ended, and its resume could not be typed ({error})"),
                    "the pane is left at its shell, and the session was not resumed",
                    "whether the pane is closing; the session can be resumed by hand",
                ),
            };
            log::warn(
                "daemon.pane.restart_failed",
                fields! {
                    "pane" => pane,
                    "program" => program,
                    "session" => session,
                    "why" => what,
                    "impact" => impact,
                    "check" => check,
                },
            );
            Reply::refused(format!("{what}; session {session} is resumed by: {line}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    const FAST: Timing = Timing {
        grace: Duration::from_millis(100),
        killed_within: Duration::from_millis(100),
        prompt_within: Duration::from_millis(200),
        quiet: Duration::from_millis(20),
        look_every: Duration::from_millis(5),
    };

    /// A pane whose program ends after the signals it ignores, and whose shell draws a prompt
    /// once it has the terminal back. Every step is written down in order.
    #[derive(Default)]
    struct Fake {
        steps: RefCell<Vec<String>>,
        /// How many signals it takes to end the program: one is SIGTERM, two is SIGKILL.
        ends_after: usize,
        signals: Cell<usize>,
        unkillable: bool,
        shell_never: bool,
        refused: Cell<bool>,
    }

    impl Fake {
        fn step(&self, step: &str) {
            self.steps.borrow_mut().push(step.to_string());
        }

        fn steps(&self) -> Vec<String> {
            self.steps.borrow().clone()
        }
    }

    impl Target for Fake {
        fn refuse_input(&self, refused: bool) {
            self.refused.set(refused);
            self.step(if refused { "refuse" } else { "accept" });
        }

        fn flush_input(&self) -> io::Result<()> {
            self.step("flush");
            Ok(())
        }

        fn terminate(&self) {
            self.signals.set(self.signals.get() + 1);
            self.step("term");
        }

        fn kill(&self) {
            self.signals.set(self.signals.get() + 1);
            self.step("kill");
        }

        fn running(&self) -> bool {
            self.unkillable || self.signals.get() < self.ends_after
        }

        fn shell_holds(&self) -> bool {
            !self.shell_never && !self.running()
        }

        fn agent_left(&self) -> bool {
            !self.running()
        }

        fn directories_said(&self) -> u64 {
            u64::from(!self.running())
        }

        fn drawn_at(&self) -> Option<Instant> {
            Some(Instant::now())
        }

        fn type_line(&self, line: &str) -> bool {
            assert!(self.refused.get(), "the line is typed while everybody else's is refused");
            self.step(&format!("type {line}"));
            true
        }
    }

    #[test]
    fn input_is_emptied_before_the_program_ends_and_again_before_the_resume_is_typed() {
        let fake = Fake { ends_after: 1, ..Fake::default() };
        assert_eq!(restart(&fake, "claude --resume s-1", FAST), Ok(Ended::Asked));
        assert_eq!(
            fake.steps(),
            ["refuse", "flush", "term", "flush", "type claude --resume s-1", "accept"]
        );
    }

    #[test]
    fn a_program_that_ignores_being_asked_is_killed_after_the_grace() {
        let fake = Fake { ends_after: 2, ..Fake::default() };
        assert_eq!(restart(&fake, "codex resume s-2", FAST), Ok(Ended::Killed));
        assert_eq!(
            fake.steps(),
            ["refuse", "flush", "term", "kill", "flush", "type codex resume s-2", "accept"]
        );
    }

    #[test]
    fn nothing_is_typed_while_the_program_runs_on_and_input_is_taken_again() {
        let fake = Fake { unkillable: true, ..Fake::default() };
        assert_eq!(restart(&fake, "claude --resume s-3", FAST), Err(Failed::StillRunning));
        assert_eq!(fake.steps(), ["refuse", "flush", "term", "kill", "accept"]);
    }

    #[test]
    fn nothing_is_typed_into_whatever_holds_the_terminal_instead_of_the_shell() {
        let fake = Fake { ends_after: 1, shell_never: true, ..Fake::default() };
        assert_eq!(restart(&fake, "claude --resume s-4", FAST), Err(Failed::ShellAway));
        assert_eq!(fake.steps(), ["refuse", "flush", "term", "accept"]);
    }

    /// A shell drawing on and on says nothing of its prompt either way, and the resume is typed
    /// once its patience is out rather than never.
    #[test]
    fn a_shell_that_never_seems_at_its_prompt_is_typed_at_once_its_patience_is_out() {
        struct Busy(Fake);
        impl Target for Busy {
            fn refuse_input(&self, refused: bool) {
                self.0.refuse_input(refused);
            }
            fn flush_input(&self) -> io::Result<()> {
                self.0.flush_input()
            }
            fn terminate(&self) {
                self.0.terminate();
            }
            fn kill(&self) {
                self.0.kill();
            }
            fn running(&self) -> bool {
                self.0.running()
            }
            fn shell_holds(&self) -> bool {
                self.0.shell_holds()
            }
            fn agent_left(&self) -> bool {
                self.0.agent_left()
            }
            fn directories_said(&self) -> u64 {
                0
            }
            fn drawn_at(&self) -> Option<Instant> {
                Some(Instant::now())
            }
            fn type_line(&self, line: &str) -> bool {
                self.0.type_line(line)
            }
        }
        let busy = Busy(Fake { ends_after: 1, ..Fake::default() });
        let started = Instant::now();
        assert_eq!(restart(&busy, "claude --resume s-5", FAST), Ok(Ended::Asked));
        assert!(started.elapsed() >= FAST.prompt_within, "it waited for the prompt");
        assert!(busy.0.steps().contains(&"type claude --resume s-5".to_string()));
    }
}
