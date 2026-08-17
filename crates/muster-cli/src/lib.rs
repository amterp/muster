//! `muster`, the command: what a script and an agent use instead of a keyboard.
//!
//! A pure client. It turns argv into a `Request`, dials a window, and renders what comes back -
//! and it decides nothing else, because everything a caller can ask for is already a message the
//! app's own chords and menu items send (architecture.md, one action path). A CLI that worked out
//! for itself which pane to split, or what a window looks like, would be a second Muster that
//! could be wrong.
//!
//! It links the schema and nothing else of Muster's. That is not tidiness: the core reaches
//! libghostty-vt through a dylib, and this is the one part of Muster somebody copies onto a
//! machine that has never heard of it.

use std::collections::BTreeMap;
use std::io::Write;

pub mod args;
pub mod dial;
pub mod docs;
pub mod environment;
pub mod render;

/// Why a run ended without an answer.
///
/// Kept apart because a caller does different things about them: a refusal means the request was
/// understood and will not happen, and an unreachable window means nobody was asked. Retrying
/// helps with exactly one of those.
#[derive(Debug)]
pub enum Trouble {
    /// This CLI or the window said no.
    Refused(String),
    /// There was no window to ask.
    Unreachable(String),
}

impl Trouble {
    /// What the process exits with, which is the only part of this a script can branch on without
    /// reading English.
    ///
    /// 2 is missing on purpose: it is clap's own code for a command line it could not read, and
    /// giving it a second meaning here would make an unparseable line and a working one
    /// indistinguishable to a script.
    pub fn code(&self) -> i32 {
        match self {
            Trouble::Refused(_) => 1,
            Trouble::Unreachable(_) => 3,
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Trouble::Refused(detail) | Trouble::Unreachable(detail) => detail,
        }
    }
}

/// One run of the command, start to finish.
///
/// Takes its argv, its environment and both streams rather than reaching for them, so that a test
/// says what it is testing - and so the one place that touches the process is `main`. The
/// exception is a command line clap refused: clap renders those itself, to the stream and in the
/// shape its own conventions call for, and re-rendering them here would be a worse version of a
/// good error.
pub fn run(
    argv: &[String],
    environment: &BTreeMap<String, String>,
    out: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    let invocation = match args::parse(argv, environment) {
        Ok(invocation) => invocation,
        Err(args::Failure::Usage(error)) => {
            let _ = error.print();
            return error.exit_code();
        }
        Err(args::Failure::Refused(refusal)) => {
            return report(&Trouble::Refused(refusal), false, errors);
        }
    };
    let json = invocation.json;

    let request = match invocation.asking {
        args::Asking::Print(text) => {
            let _ = writeln!(out, "{}", text.trim_end());
            return 0;
        }
        args::Asking::Send(request) => request,
    };

    let rendered = dial::ask(&request, invocation.socket.as_deref(), environment)
        .and_then(|response| render::answer(&response, json));
    match rendered {
        Ok(text) => {
            if !text.is_empty() {
                let _ = writeln!(out, "{}", text.trim_end());
            }
            0
        }
        Err(trouble) => report(&trouble, json, errors),
    }
}

/// Refusals go to stderr, in whichever shape was asked for.
///
/// stderr rather than stdout even under `--json`, so that a caller reading stdout gets the answer
/// or nothing - a script that piped an error object into `jq` and got a field it did not expect is
/// worse off than one that got nothing and a non-zero exit.
fn report(trouble: &Trouble, json: bool, errors: &mut impl Write) -> i32 {
    if json {
        let _ = writeln!(errors, "{}", serde_json::json!({ "error": trouble.detail() }));
    } else {
        let _ = writeln!(
            errors,
            "{}muster{}: {}",
            render::ERROR.render(),
            render::ERROR.render_reset(),
            trouble.detail()
        );
    }
    trouble.code()
}
