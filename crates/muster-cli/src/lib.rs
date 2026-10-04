//! `muster`, the command: what a script and an agent use instead of a keyboard.
//!
//! A pure client. It turns argv into a `Request`, dials a window, and renders what comes back -
//! and it decides nothing else, because everything a caller can ask for is already a message the
//! app's own chords and menu items send (architecture.md, one action path). A CLI that worked out
//! for itself which pane to split, or what a window looks like, would be a second Muster that
//! could be wrong.
//!
//! The one exception is a window that is not there. Then what agents are doing, what a pane
//! printed, typing into one and waiting on one are asked of this machine's daemon instead
//! (`windowless`), by the same rules the window applies, which is why they live where both can
//! reach them rather than here.
//!
//! It links the schemas and the core's pure rules, and nothing that reaches libghostty. That is
//! not tidiness: libghostty-vt is a dylib the app and the daemon carry, and this is the one part
//! of Muster somebody copies onto a machine that has never heard of it.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

pub mod args;
pub mod daemon;
pub mod diagram;
pub mod dial;
pub mod docs;
pub mod environment;
pub mod messaging;
pub mod opening;
mod redraw;
pub mod render;
pub mod windowless;

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
    /// A window took the request and this command cannot say what came of it - because the
    /// window never answered, or because the daemon behind it never answered the window.
    Unanswered(String),
    /// [`Trouble::Unanswered`], for a request that makes a pane: the pane it may have made, under
    /// the name the window gave it, so a caller can find it or close it rather than make another.
    MayHaveMade { reason: String, pane: String },
    /// A wait ran out before what it was waiting for happened. Waiting changes nothing, so
    /// waiting again is harmless.
    TimedOut(String),
    /// Done, with nobody there to hear it: a message posted that woke no live participant
    /// (MIP-4, section 4). The answer is printed as any other, so a script that only wants it
    /// posted reads it the same way, and one that wants it heard branches on the code.
    Unheard(String),
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
            Trouble::Unanswered(_) | Trouble::MayHaveMade { .. } => 4,
            Trouble::TimedOut(_) => 5,
            Trouble::Unheard(_) => 6,
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Trouble::Refused(detail)
            | Trouble::Unreachable(detail)
            | Trouble::Unanswered(detail)
            | Trouble::MayHaveMade { reason: detail, .. }
            | Trouble::TimedOut(detail)
            | Trouble::Unheard(detail) => detail,
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
// One arm per kind of command, which is the dispatch read in one place.
#[allow(clippy::too_many_lines)]
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
    let no_window = invocation.no_window;

    let request = match invocation.asking {
        args::Asking::Print(text) => {
            let _ = writeln!(out, "{}", text.trim_end());
            return 0;
        }
        asking @ (args::Asking::MakeWindow(_)
        | args::Asking::ReopenWindow(_)
        | args::Asking::CloseWindow(_)) => {
            return match about_a_window(&asking, named.as_deref(), environment) {
                Ok(answer) => {
                    // The name alone, with nothing around it, for the reason `pane new` prints a
                    // bare pane name: it is what the next command takes. The socket reaches the
                    // app rather than one window, so it is in the JSON for a script that wants it.
                    let _ = writeln!(
                        out,
                        "{}",
                        if json {
                            answer.to_string()
                        } else {
                            answer["window"].as_str().unwrap_or_default().to_string()
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
            for note in context_unsaid(&request, named.as_deref(), no_window, environment) {
                say_note(&note, json, errors);
            }
            let answers = follow(&request, named.as_deref(), no_window, environment);
            return watch(&request, timeout, answers, json, out, errors);
        }
        args::Asking::WatchLayout { watch, read } => {
            return redraw::run(
                &watch,
                &read,
                named.as_deref(),
                no_window,
                environment,
                json,
                out,
                errors,
            );
        }
        args::Asking::Survey { closed } => {
            let answers = dial::survey(environment, &read_window());
            let here = environment.get(environment::WINDOW_SOCKET).filter(|path| !path.is_empty());
            let text = render::windows(&answers, here.map(String::as_str), closed, json);
            let _ = writeln!(out, "{}", text.trim_end());
            return 0;
        }
        args::Asking::Message(messaging) if messaging.follow => {
            return match messaging::follow(&messaging, environment, json, out) {
                Ok(()) => 0,
                Err(trouble) => report(&trouble, json, errors),
            };
        }
        args::Asking::Message(messaging) => {
            let rendered = messaging::run(*messaging, environment, input, json);
            return finish(rendered, json, out, errors);
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

    if no_window {
        return finish(from_the_daemon(&request, environment, json), json, out, errors);
    }

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
        let answers = held_by_one(&request, dial::survey_around(environment, &request));
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

    let rendered = ask_a_window(&request, named.as_deref(), environment, json);
    finish(rendered, json, out, errors)
}

/// Opens, reopens or closes a window, and answers what it printed under `--json`.
fn about_a_window(
    asking: &args::Asking,
    socket: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> Result<serde_json::Value, Trouble> {
    let opened = match asking {
        args::Asking::CloseWindow(name) => {
            let closed = opening::close_a_window(environment, socket, name.as_deref())?;
            return Ok(serde_json::json!({ "window": closed }));
        }
        args::Asking::ReopenWindow(name) => {
            opening::the_closed_window(environment, name.as_deref())
        }
        args::Asking::MakeWindow(onto) => opening::another_window(environment, onto),
        _ => unreachable!("only the window verbs reach here"),
    }?;
    Ok(serde_json::json!({ "window": opened.window, "socket": opened.socket }))
}

/// The one answer among several from the app holding what the request names, when exactly one
/// holds it: a read about one pane or tab is refused by every other app, and the holder's answer
/// is the answer, as it would be with one app listening. Every answer otherwise.
fn held_by_one(
    request: &muster_proto::Request,
    mut answers: Vec<(String, Result<muster_proto::Response, Trouble>)>,
) -> Vec<(String, Result<muster_proto::Response, Trouble>)> {
    let names_one = request.payload.as_ref().is_some_and(|payload| {
        muster_proto::names(payload).is_some()
            || matches!(payload, muster_proto::request::Payload::ReadPane(read)
                if !read.pane_id.is_empty())
    });
    if !names_one {
        return answers;
    }
    let holds = |answer: &Result<muster_proto::Response, Trouble>| {
        answer.as_ref().is_ok_and(|response| {
            !matches!(response.payload, Some(muster_proto::response::Payload::Failure(_)))
        })
    };
    if answers.iter().filter(|(_, answer)| holds(answer)).count() == 1 {
        answers.retain(|(_, answer)| holds(answer));
    }
    answers
}

/// Asks the window the caller means, or this machine's daemon when there is no window at all.
fn ask_a_window(
    request: &muster_proto::Request,
    named: Option<&str>,
    environment: &BTreeMap<String, String>,
    json: bool,
) -> Result<String, Trouble> {
    match dial::ask(request, named, environment) {
        Err(Trouble::Unreachable(detail)) if no_window_at_all(request, named, environment) => {
            if windowless::can_answer(request) {
                from_the_daemon(request, environment, json)
                    .map_err(|trouble| neither(&detail, trouble))
            } else {
                Err(Trouble::Unreachable(format!("{detail} {WITHOUT_A_WINDOW}")))
            }
        }
        asked => asked.and_then(|response| render::answer(&response, json)),
    }
}

/// Whether a request that reached no window found none at all, rather than one that would not
/// answer or several to choose from: then this machine's daemon, which holds the panes, may
/// answer in its place.
pub(crate) fn no_window_at_all(
    request: &muster_proto::Request,
    named: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> bool {
    named.is_none() && request.window.is_empty() && !dial::any_window_answers(environment)
}

/// Why a request found no window, and then no daemon either: both, since the first is what a
/// caller can usually act on.
pub(crate) fn neither(window: &str, daemon: Trouble) -> Trouble {
    match daemon {
        Trouble::Unreachable(daemon) => Trouble::Unreachable(format!(
            "{window} This machine's muster-daemon could have answered instead, and did not \
             either: {daemon}"
        )),
        other => other,
    }
}

/// What a refusal for want of a window adds, so a caller learns what still works.
const WITHOUT_A_WINDOW: &str = "Without one, `muster window`, `pane read`, `pane send` and \
    `pane wait` still work, answered by this machine's muster-daemon; everything else is about \
    what a window shows or how it lays out its tabs.";

/// Asks this machine's daemon what a window was asked, and renders what it says the way a
/// window's answer is rendered.
fn from_the_daemon(
    request: &muster_proto::Request,
    environment: &BTreeMap<String, String>,
    json: bool,
) -> Result<String, Trouble> {
    if !windowless::can_answer(request) {
        return Err(Trouble::Refused(format!(
            "--no-window asks this machine's muster-daemon, and this needs a window. \
             {WITHOUT_A_WINDOW}"
        )));
    }
    match windowless::ask(request, environment)? {
        windowless::Answered::Response(response) => render::answer(&response, json),
        windowless::Answered::Window { window, socket } => {
            Ok(render::daemon_window(&window, &socket, json))
        }
    }
}

/// Where a watch's answers come from: the window, or this machine's daemon when none answers
/// or the caller said `--no-window`.
fn follow(
    request: &muster_proto::Request,
    named: Option<&str>,
    no_window: bool,
    environment: &BTreeMap<String, String>,
) -> Result<Box<dyn Answers>, Trouble> {
    let from_the_daemon =
        || windowless::follow(request, environment).map(|watching| Box::new(watching) as _);
    if no_window {
        return from_the_daemon();
    }
    match dial::follow(request, named, environment) {
        Err(Trouble::Unreachable(detail)) if no_window_at_all(request, named, environment) => {
            from_the_daemon().map_err(|trouble| neither(&detail, trouble))
        }
        followed => followed.map(|answers| Box::new(answers) as _),
    }
}

/// What a wait on context says before it begins, about each pane it names whose agent has not
/// said how full its context is: such a wait can still be met, once the agent says, and a caller
/// whose harness never will should hear that now rather than after its timeout.
///
/// Read from the window, or the daemon in its place, the way `muster window` would be. Advice
/// rather than a check, so a read that fails says nothing and leaves the wait to say what is
/// wrong.
fn context_unsaid(
    request: &muster_proto::Request,
    named: Option<&str>,
    no_window: bool,
    environment: &BTreeMap<String, String>,
) -> Vec<String> {
    let Some(muster_proto::request::Payload::WatchPanes(watch)) = request.payload.as_ref() else {
        return Vec::new();
    };
    if watch.context_at_least.is_none() {
        return Vec::new();
    }
    let mut read = muster_proto::Request::new(muster_proto::request::Payload::ReadWindow(
        muster_proto::ReadWindow::default(),
    ));
    read.window.clone_from(&request.window);
    let window = if no_window {
        windowless::ask(&read, environment).ok()
    } else {
        match dial::ask(&read, named, environment) {
            Ok(response) => Some(windowless::Answered::Response(Box::new(response))),
            Err(Trouble::Unreachable(_)) if no_window_at_all(&read, named, environment) => {
                windowless::ask(&read, environment).ok()
            }
            Err(_) => None,
        }
    };
    let panes = match window {
        Some(windowless::Answered::Window { window, .. }) => window.panes,
        Some(windowless::Answered::Response(response)) => match response.payload {
            Some(muster_proto::response::Payload::Window(window)) => window.panes,
            _ => return Vec::new(),
        },
        None => return Vec::new(),
    };
    watch
        .pane_ids
        .iter()
        .filter(|named| {
            panes.iter().any(|pane| {
                &pane.pane_id == *named
                    && pane.facts.as_ref().is_none_or(|facts| facts.context_used.is_none())
            })
        })
        .map(|pane| {
            format!(
                "{pane} has not said how full its context is. Claude Code, Codex and OpenCode say \
                 it once Muster's adapter is installed (`muster docs harnesses`); until {pane} \
                 does, --context cannot end this wait."
            )
        })
        .collect()
}

/// Something worth knowing that is not an answer, on stderr so a script reading the answers
/// does not read it.
fn say_note(note: &str, json: bool, errors: &mut impl Write) {
    if json {
        let _ = writeln!(errors, "{}", serde_json::json!({ "note": note }));
    } else {
        let _ = writeln!(errors, "muster: {note}");
    }
}

/// A stream of answers to a watch, from a window or from this machine's daemon.
///
/// `Send` so a layout being drawn again can read it on a thread of its own.
trait Answers: Send {
    fn next(
        &mut self,
        deadline: Option<std::time::Instant>,
    ) -> Result<muster_proto::Response, dial::Ended>;
}

impl Answers for dial::Answers {
    fn next(
        &mut self,
        deadline: Option<std::time::Instant>,
    ) -> Result<muster_proto::Response, dial::Ended> {
        dial::Answers::next(self, deadline)
    }
}

impl Answers for windowless::Watching {
    fn next(
        &mut self,
        deadline: Option<std::time::Instant>,
    ) -> Result<muster_proto::Response, dial::Ended> {
        windowless::Watching::next(self, deadline)
    }
}

/// Prints an answer, or reports why there is none, and says what to exit with.
fn finish(
    rendered: Result<String, Trouble>,
    json: bool,
    out: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    match rendered {
        Ok(text) => {
            if !text.is_empty() {
                let _ = writeln!(out, "{}", text.trim_end());
            }
            0
        }
        Err(Trouble::Unheard(text)) => {
            let _ = writeln!(out, "{}", text.trim_end());
            6
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
    answers: Result<Box<dyn Answers>, Trouble>,
    json: bool,
    out: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    let deadline = timeout.map(|timeout| std::time::Instant::now() + timeout);
    let on_context = matches!(
        request.payload.as_ref(),
        Some(muster_proto::request::Payload::WatchPanes(watch)) if watch.context_at_least.is_some()
    );
    let mut answers = match answers {
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
                let line = if on_context { with_context(line, &response, json) } else { line };
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

/// A pane that met a wait on context, with the context it said: the state alone would not say
/// why a wait for a full context ended on an agent still working.
fn with_context(line: String, response: &muster_proto::Response, json: bool) -> String {
    let Some(muster_proto::response::Payload::PaneState(agent)) = response.payload.as_ref() else {
        return line;
    };
    let Some(used) = agent.facts.as_ref().and_then(|facts| facts.context_used) else {
        return line;
    };
    if !json {
        return format!("{line}  {used:.0}% context");
    }
    match serde_json::from_str::<serde_json::Value>(&line) {
        Ok(mut value) => {
            value["context_used"] = used.into();
            value.to_string()
        }
        Err(_) => line,
    }
}

/// What a wait that ran out says, naming what it was waiting for.
fn ran_out(request: &muster_proto::Request, waited: u64) -> String {
    let Some(muster_proto::request::Payload::WatchPanes(watch)) = request.payload.as_ref() else {
        return format!("nothing arrived within {waited}s.");
    };
    let until = muster_core::Until::parse(&watch.until, watch.context_at_least)
        .map_or_else(|_| watch.until.join(" or "), |until| until.spelled());
    format!(
        "{} {} not {until} within {waited}s. Waiting changed nothing, so waiting again is \
         harmless; `muster window` says what {} doing now.",
        watch.pane_ids.join(", "),
        if watch.pane_ids.len() == 1 { "was" } else { "were" },
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
        let mut error = serde_json::json!({ "error": trouble.detail() });
        if let Trouble::MayHaveMade { pane, .. } = trouble {
            error["pane"] = pane.clone().into();
        }
        let _ = writeln!(errors, "{error}");
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
/// command run where somebody is working already knows which window it is about. So this
/// reaches a caller standing outside every pane, and one in a pane whose window has quit, which
/// asks the windows open beside it (`dial::around`).
fn asks_around(
    request: &muster_proto::Request,
    named: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> bool {
    let in_a_pane =
        environment.get(environment::WINDOW_SOCKET).is_some_and(|path| !path.is_empty());
    if named.is_some()
        || !request.window.is_empty()
        || (in_a_pane && !dial::own_window_gone(environment))
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
    muster_proto::Request::new(muster_proto::request::Payload::ReadWindow(
        muster_proto::ReadWindow::default(),
    ))
}
