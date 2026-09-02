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
pub mod opening;
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
    // Read before the match takes the request out of it, so that "did the caller name a window"
    // is still answerable below.
    let named = invocation.socket.clone();

    let request = match invocation.asking {
        args::Asking::Print(text) => {
            let _ = writeln!(out, "{}", text.trim_end());
            return 0;
        }
        args::Asking::MakeWindow | args::Asking::ReopenWindow => {
            // Two verbs and one act. Which arrangement the new window takes is the whole
            // difference, and the window itself works that out from the flag it is launched
            // with - so this picks the launch and nothing here decides anything else.
            let opened = if matches!(invocation.asking, args::Asking::MakeWindow) {
                opening::another_window(environment)
            } else {
                opening::the_closed_window(environment)
            };
            return match opened {
                Ok(socket) => {
                    // The socket alone, with nothing around it, for the reason `pane new` prints
                    // a bare pane name: the next line is
                    // `muster --socket "$(muster window new)" pane new --run claude`.
                    let _ = writeln!(
                        out,
                        "{}",
                        if json {
                            serde_json::json!({ "socket": socket }).to_string()
                        } else {
                            socket
                        }
                    );
                    0
                }
                Err(trouble) => report(&trouble, json, errors),
            };
        }
        args::Asking::Survey => {
            let answers = dial::survey(environment, &read_window());
            let here = environment.get(environment::WINDOW_SOCKET).filter(|path| !path.is_empty());
            let text = render::windows(&answers, here.map(String::as_str), json);
            let _ = writeln!(out, "{}", text.trim_end());
            return 0;
        }
        args::Asking::Send(request) => request,
    };

    // A question nobody narrowed, with more than one window listening. Naming no window is a
    // real problem for a write - `pane new` has to know which window it makes a pane in - and no
    // problem at all for a read, where "what is everything doing" wants all of them. Writes go
    // on refusing, and the refusal names the sockets.
    //
    // Only when several answer. One window prints exactly what it printed before this existed,
    // which is what keeps every script that reads `muster window --json` working; none falls
    // through to `ask`, so the message about there being no window to talk to stays the one that
    // command already wrote.
    if asks_around(&request, named.as_deref(), environment) {
        let answers = dial::survey(environment, &request);
        if answers.len() > 1 {
            let text = render::answers(&answers, json);
            let _ = writeln!(out, "{}", text.trim_end());
            return 0;
        }
        if let Some((_, answer)) = answers.into_iter().next() {
            return match answer.and_then(|response| render::answer(&response, json)) {
                Ok(text) => {
                    if !text.is_empty() {
                        let _ = writeln!(out, "{}", text.trim_end());
                    }
                    0
                }
                Err(trouble) => report(&trouble, json, errors),
            };
        }
    }

    let rendered = dial::ask(&request, named.as_deref(), environment)
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

/// Whether this command may be asked of every window rather than of one.
///
/// Two conditions, and both are about what the caller said rather than about how many windows
/// there are. It has to be a question, which `muster_proto::only_reads` decides and the window
/// itself reads for a different purpose. And the caller has to have named no window: `--socket`
/// and `$MUSTER_SOCKET` each mean one, and the second is set in every pane Muster makes - so a
/// command run where somebody is working already knows which window it is about, and this
/// reaches only a caller standing outside every pane.
fn asks_around(
    request: &muster_proto::Request,
    named: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> bool {
    if named.is_some()
        || environment.get(environment::WINDOW_SOCKET).is_some_and(|path| !path.is_empty())
    {
        return false;
    }
    request.payload.as_ref().is_some_and(muster_proto::only_reads)
}

/// The request that asks a window what it is showing.
///
/// Built here as well as in `args` because two commands that name no window still have to ask
/// one something: listing windows asks every window this, and making one asks a window that has
/// only just appeared whether it is ready to be handed to a caller.
fn read_window() -> muster_proto::Request {
    muster_proto::Request {
        payload: Some(muster_proto::request::Payload::ReadWindow(muster_proto::ReadWindow {})),
    }
}
