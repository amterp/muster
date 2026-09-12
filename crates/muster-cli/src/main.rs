//! The one place that touches the process.
//!
//! Everything else takes its argv, its environment and its output as parameters, so that the whole
//! of the CLI is testable without spawning anything.

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let environment = std::env::vars().collect();
    // The process's own answer rather than $PWD, which a parent that never updated it leaves
    // pointing at a directory this command is not in - the same silently-wrong-directory this
    // is here to resolve. `None` when there is no answer, which is a working directory that has
    // been deleted underneath the shell.
    let here = std::env::current_dir().ok();
    // anstream decides here, once, whether the escapes the renderer writes survive: kept for a
    // terminal, stripped for a pipe, a file, `NO_COLOR`, or a `TERM` that cannot draw them. So
    // nothing downstream branches on it, and a person and an agent read the same code path.
    let code = muster_cli::run(
        &argv,
        &environment,
        here.as_deref(),
        &mut std::io::stdin().lock(),
        &mut anstream::stdout(),
        &mut anstream::stderr(),
    );
    std::process::exit(code);
}
