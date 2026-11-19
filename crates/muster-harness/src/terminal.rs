/// Shell text that sets a pane's terminal with `stty <settings>`, run a second time when the
/// first is stopped.
///
/// A pane's command runs in a job-control shell (Fencil `pane-command-job-control`), which
/// moves every command it forks into a process group of its own. On macOS 26.4.1 about one fork
/// in a thousand ends up outside the terminal's foreground group, and its first change to the
/// terminal stops it with SIGTTOU. The shell reports the job stopped and goes on, so a test
/// program printed its ready line with the terminal still canonical, and the input sent to it
/// waited for a newline until the test gave up. A stopped `stty` exits 150 to the shell, and
/// the second runs in a fresh fork.
///
/// Braced so that it stays one command wherever it is spliced. Use it directly in the pane's
/// command, not under a nested `sh -c`: a shell without job control waits on a stopped child
/// for good, and the second `stty` never runs.
pub fn stty(settings: &str) -> String {
    format!("{{ stty {settings} || stty {settings}; }}")
}
