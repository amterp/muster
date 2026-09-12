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
use std::io::{Read, Write};
use std::path::Path;

pub mod args;
pub mod dial;
pub mod docs;
pub mod environment;
pub mod opening;
pub mod render;

/// Why a run ended without an answer.
///
/// Kept apart because a caller does different things about them, and the difference is about
/// whether the request happened rather than about what went wrong. Nobody was asked, so sending
/// it again costs nothing. It was refused, so sending it again earns the same refusal. Or it
/// landed and what came of it is unknown - and that is the one where sending it again is how a
/// pane receives the same instruction twice.
#[derive(Debug)]
pub enum Trouble {
    /// This CLI or the window said no.
    Refused(String),
    /// There was no window to ask.
    Unreachable(String),
    /// A window took the request and this command cannot say what came of it.
    Unanswered(String),
    /// A wait ran out before what it was waiting for happened. Waiting changes nothing, so
    /// waiting again is harmless.
    TimedOut(String),
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
            Trouble::Unanswered(_) => 4,
            Trouble::TimedOut(_) => 5,
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Trouble::Refused(detail)
            | Trouble::Unreachable(detail)
            | Trouble::Unanswered(detail)
            | Trouble::TimedOut(detail) => detail,
        }
    }
}

/// One run of the command, start to finish.
///
/// Takes its argv, its environment, the directory it was run in and all three streams rather
/// than reaching for them, so that a test says what it is testing - and so the one place that
/// touches the process is `main`. The exception is a command line clap refused: clap renders
/// those itself, to the stream and in the shape its own conventions call for, and re-rendering
/// them here would be a worse version of a good error.
pub fn run(
    argv: &[String],
    environment: &BTreeMap<String, String>,
    here: Option<&Path>,
    input: &mut impl Read,
    out: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    let invocation = match args::parse(argv, environment, here) {
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
        args::Asking::Watch { request, timeout } => {
            // Never asked around, unlike a read. A watch is one connection held open, and a
            // caller with several windows listening names one with --socket.
            return watch(&request, timeout, named.as_deref(), environment, json, out, errors);
        }
        args::Asking::Survey => {
            let answers = dial::survey(environment, &read_window());
            let here = environment.get(environment::WINDOW_SOCKET).filter(|path| !path.is_empty());
            let text = render::windows(&answers, here.map(String::as_str), json);
            let _ = writeln!(out, "{}", text.trim_end());
            return 0;
        }
        args::Asking::Send(request) => request,
        args::Asking::SendFrom { mut request, from } => match read_text(&from, input) {
            Ok(text) => {
                if let Some(muster_proto::request::Payload::SendToPane(send)) =
                    request.payload.as_mut()
                {
                    send.text = text;
                }
                request
            }
            Err(trouble) => return report(&trouble, json, errors),
        },
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

/// Prints a watch's answers as they arrive, and exits with how it ended.
///
/// Each line is flushed as it is written, because the reader is usually a pipe - a Monitor, a
/// `while read` loop - that is acting on each line as it comes, and a line sitting in a buffer
/// is a change nobody hears about.
fn watch(
    request: &muster_proto::Request,
    timeout: Option<std::time::Duration>,
    socket: Option<&str>,
    environment: &BTreeMap<String, String>,
    json: bool,
    out: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    let deadline = timeout.map(|timeout| std::time::Instant::now() + timeout);
    let mut answers = match dial::follow(request, socket, environment) {
        Ok(answers) => answers,
        Err(trouble) => return report(&trouble, json, errors),
    };
    loop {
        let response = match answers.next(deadline) {
            Ok(response) => response,
            Err(dial::Ended::HungUp(detail)) => {
                return report(&Trouble::Unreachable(detail), json, errors);
            }
            Err(dial::Ended::TimedOut) => {
                let waited = timeout.map_or(0, |timeout| timeout.as_secs());
                return report(&Trouble::TimedOut(ran_out(request, waited)), json, errors);
            }
        };
        if matches!(response.payload, Some(muster_proto::response::Payload::Ok(_))) {
            return 0;
        }
        match render::answer(&response, json) {
            Ok(line) => {
                // The reader went away - `| head -1`, a Monitor stopped. Nothing is wrong, and
                // there is nobody left to say anything to.
                if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
                    return 0;
                }
            }
            Err(trouble) => return report(&trouble, json, errors),
        }
    }
}

/// What a wait that ran out says, naming what it was waiting for.
fn ran_out(request: &muster_proto::Request, waited: u64) -> String {
    let Some(muster_proto::request::Payload::WatchPanes(watch)) = request.payload.as_ref() else {
        return format!("nothing arrived within {waited}s.");
    };
    format!(
        "{} {} not {} within {waited}s. Waiting changed nothing, so waiting again is harmless; \
         `muster window` says what {} doing now.",
        watch.pane_ids.join(", "),
        if watch.pane_ids.len() == 1 { "was" } else { "were" },
        watch.until.join(" or "),
        if watch.pane_ids.len() == 1 { "it is" } else { "they are" },
    )
}

/// The text a `pane send --file` or `pane send -` types, read before anything is dialed.
///
/// A failure here is a refusal: nothing has been asked of a window yet, so there is nothing
/// that could have happened.
fn read_text(from: &args::TextSource, input: &mut impl Read) -> Result<String, Trouble> {
    let bytes = match from {
        args::TextSource::File(path) => std::fs::read(path).map_err(|error| {
            Trouble::Refused(format!("could not read {path} ({error}), so nothing was sent."))
        })?,
        args::TextSource::Stdin => {
            let mut bytes = Vec::new();
            input.read_to_end(&mut bytes).map_err(|error| {
                Trouble::Refused(format!(
                    "could not read the text from stdin ({error}), so nothing was sent."
                ))
            })?;
            bytes
        }
    };
    args::text_of(bytes, from).map_err(Trouble::Refused)
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
