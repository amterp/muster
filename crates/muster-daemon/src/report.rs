//! `muster-daemon report`: what the agent in a pane says about itself, told to the daemon that
//! owns the pane.
//!
//! Run from inside a pane - by a harness's hooks and statusline - it finds its pane and its
//! daemon in the environment the daemon gave the pane (`MUSTER_PANE`, `MUSTER_DAEMON_SOCKET`),
//! so it needs no window and works on a machine where the daemon is the only piece of Muster
//! installed. It is quick and quiet: one request, a short timeout, and nothing printed unless
//! the daemon did not take the report.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use muster_daemon_proto::{self as proto, ConnectionKind, connection, pane_request};

use crate::session::HANDING_OVER;

/// The most JSON `--from` reads from stdin. A statusline's or a hook's input is a few kilobytes.
const STDIN_BYTES: u64 = 1 << 20;

/// The flags `--from` may fill: each one that takes one value.
const FROM_FLAGS: [&str; 12] = [
    "context-used",
    "context-session",
    "model",
    "effort",
    "cost-usd",
    "waiting",
    "session-name",
    "session-id",
    "session-source",
    "compacted-in",
    "agent",
    "state",
];

/// How long a report waits for its daemon, from dialing to the answer. A hook that runs on every
/// sub-agent and a statusline that runs on every message must not hold their harness up.
const PATIENCE: Duration = Duration::from_secs(2);

/// How long a report saying what the agent waits on waits for its daemon. An agent runs it once,
/// as it ends its turn, so nothing is held up by a longer wait, and a wait it believed declared
/// and was not leaves it asleep with nothing to wake it.
const WAITING_PATIENCE: Duration = Duration::from_secs(10);

/// Why a report was not confirmed, by what its sender should do next.
#[derive(Debug)]
enum Unreported {
    /// Nothing reached the daemon, or it refused the report: reporting again is safe.
    NotReported(String),
    /// The daemon took the report and the answer never came, so it may have landed.
    AnswerLost(String),
}

const USAGE: &str = "usage: muster-daemon report [--pane PANE] [--context-used PERCENT] \
    [--context-session ID] [--model NAME] [--effort LEVEL] [--cost-usd DOLLARS] [--subagent-started | --subagent-stopped] \
    [--fact KEY=VALUE]... [--waiting TEXT [--until-exits PID | --until-message-from PANE]] [--clear] \
    [--agent NAME [--state working|blocked|idle] [--session-name NAME] \
    [--session-id ID] [--session-source SOURCE] [--compacting] \
    [--context-compacted | --compacted-in TRANSCRIPT]] [--from FLAG=POINTER]...\n\n\
    Tells the daemon that owns this pane what the agent in it says about itself. The pane is \
    $MUSTER_PANE unless --pane names another, by its id, callsign or name, and the daemon is the one at \
    $MUSTER_DAEMON_SOCKET. --effort is the reasoning effort the model runs at, as the harness \
    names it (high). An empty model, effort or fact value removes it; --clear forgets everything \
    reported before, and applies first. --context-session is the harness's id for the session \
    the context and cost belong to: a report naming another session than the last forgets the \
    context and cost reported before it, as a statusline naming a new session after /clear \
    does. It does not name the pane's session, as --session-id does. --waiting says what the agent ended its turn to wait \
    on, work it started itself; it lasts until a later turn ends without saying it again, and \
    an empty one clears it. Say it in each turn that ends to wait. A prompt Muster gives the \
    agent, a message's wake or a compaction, does not end it, and a wait nothing is known to \
    end lasts 10 minutes at most. --until-exits ends the wait when process \
    PID exits instead, and the daemon then tells the agent in a message, which wakes it: for a \
    harness nothing else wakes when its work ends. Turns do not end such a wait. \
    --until-message-from ends the wait when a message from that pane is delivered; unrelated \
    senders leave it standing. Give a pane id, callsign or unique name, including on a linked \
    machine. It cannot be combined with --until-exits. --state is the \
    agent's own word on what it is doing, which outranks what detection reads off its screen while fresh; --agent names the agent, \
    as its detection manifest does (claude), and the state counts only while that agent is \
    the pane's. --session-name is what the agent's harness calls its session, empty for no \
    name: the pane takes a name the session is given once it has started, and gives the \
    session the pane's own name otherwise. It needs --agent, as --state does. --session-id is \
    the harness's id for its session, which the daemon names it by to reach it through the \
    harness's own command, where its manifest says how; empty forgets it. It needs --agent too. \
    --compacting says the agent's harness began compacting its context by itself, so the turn it \
    shows as is not the agent going to work; it needs --agent too. --session-source is what \
    started the session, in the words of a hook's `source`: clear says the agent's context was \
    cleared, startup says so too when the agent still held another session, which --session-id \
    in the same report settles, and anything else says nothing. --context-compacted says the \
    agent's context was compacted. --compacted-in says so about a Claude Code transcript, which \
    has to record the compaction first: a sub-agent's fires the same hooks as its parent's and \
    records it elsewhere, so the report waits a few seconds for the transcript and sends nothing \
    if it stays silent. All three need --agent. \
    --from fills a flag from the JSON object on stdin, which a harness hands its hooks and \
    statusline: --from context-used=/context_window/used_percentage reads that JSON pointer, a \
    number or text, as --context-used's value. A value missing or null leaves the flag out, and \
    a flag given later replaces one given before, so --session-name '' before a --from \
    session-name says no name when the JSON has none, and --from session-id=/session_id reads \
    the id a hook is handed. Stdin is read only for --from, and never from a terminal. A report \
    whose every --from found nothing, and that says nothing else, is not sent. The daemon has \
    2 seconds to answer, or 10 with --waiting. Exit 1 says nothing was reported and reporting \
    again is safe; exit 4 says the daemon took the report and did not answer, so it may have \
    landed: read `muster window` before reporting again.";

pub(crate) fn run(arguments: impl Iterator<Item = String>) -> ExitCode {
    let Filled { arguments, came_up_empty } = match filled_from(arguments.collect(), read_stdin) {
        Ok(filled) => filled,
        Err(problem) => {
            eprintln!("muster-daemon report: {problem}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let (mut report, compaction_in) =
        match parse(arguments.into_iter(), |name| std::env::var(name).ok()) {
            Ok(Parsed::Report(report, compaction_in)) => (*report, compaction_in),
            Ok(Parsed::Help) => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Err(problem) => {
                eprintln!("muster-daemon report: {problem}\n{USAGE}");
                return ExitCode::from(2);
            }
        };
    // A hook whose input held none of what it reads from it - a session start that carried no id
    // - has nothing to tell, and the daemon is not dialed for it. A report with no --from at all
    // is sent as it stands: the daemon counts any report it takes as the adapter reporting.
    if came_up_empty && says_nothing(&report) {
        return ExitCode::SUCCESS;
    }
    if let Some(transcript) = compaction_in {
        let started = crate::state_since::now_ms();
        let Some(boundary) = crate::transcript::recorded_compaction(&transcript, started) else {
            return ExitCode::SUCCESS;
        };
        report.context_change_id = Some(boundary);
    }
    let Some(socket) = std::env::var_os("MUSTER_DAEMON_SOCKET").map(PathBuf::from) else {
        eprintln!(
            "muster-daemon report: MUSTER_DAEMON_SOCKET is not set, so this is not running in a \
             pane of a Muster daemon and there is nobody to tell"
        );
        return ExitCode::FAILURE;
    };
    let patience = if report.waiting.is_some() { WAITING_PATIENCE } else { PATIENCE };
    match send_within(socket, report, patience) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Unreported::NotReported(problem)) => {
            eprintln!("muster-daemon report: {problem}");
            ExitCode::FAILURE
        }
        // Exit 4 as `muster` uses it: the request may have happened, so do not repeat it blind.
        Err(Unreported::AnswerLost(problem)) => {
            eprintln!(
                "muster-daemon report: {problem}, so the report may have landed. Read `muster \
                 window` before reporting again: a repeated --subagent-started or \
                 --subagent-stopped counts twice."
            );
            ExitCode::from(4)
        }
    }
}

/// The arguments with each `--from FLAG=POINTER` replaced by `--FLAG VALUE`, the value read out of
/// the JSON object `stdin` gives - read once, and only when a `--from` asks for it - or by nothing
/// where the JSON holds no value there. Kept in place, so that order decides between a `--from`
/// and the same flag given outright.
fn filled_from(
    arguments: Vec<String>,
    stdin: impl FnOnce() -> Result<String, String>,
) -> Result<Filled, String> {
    if !arguments.iter().any(|argument| argument == "--from") {
        return Ok(Filled { arguments, came_up_empty: false });
    }
    let json: serde_json::Value = serde_json::from_str(&stdin()?).map_err(|error| {
        format!("--from reads a JSON object on stdin, and it is not one: {error}")
    })?;
    let mut filled = Vec::with_capacity(arguments.len());
    let mut found = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument != "--from" {
            filled.push(argument);
            continue;
        }
        let given = arguments.next().ok_or("--from needs a value")?;
        let (flag, pointer) =
            given.split_once('=').ok_or(format!("--from {given} is not FLAG=POINTER"))?;
        if !FROM_FLAGS.contains(&flag) {
            return Err(format!(
                "--from {given} names {flag}, and --from fills only {}",
                FROM_FLAGS.join(", ")
            ));
        }
        let value = match json.pointer(pointer) {
            None | Some(serde_json::Value::Null) => continue,
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(serde_json::Value::Number(number)) => number.to_string(),
            Some(other) => {
                return Err(format!("--from {given} found {other}, which is not text or a number"));
            }
        };
        filled.extend([format!("--{flag}"), value]);
        found = true;
    }
    Ok(Filled { arguments: filled, came_up_empty: !found })
}

/// The arguments with every `--from` filled in, and whether there were some and every one of them
/// found nothing.
struct Filled {
    arguments: Vec<String>,
    came_up_empty: bool,
}

/// Whether a report holds nothing beyond its pane and the agent sending it.
fn says_nothing(report: &pane_request::Report) -> bool {
    *report
        == pane_request::Report {
            pane: report.pane.clone(),
            agent: report.agent.clone(),
            ..pane_request::Report::default()
        }
}

/// A harness's JSON, from standard input. Refused at a terminal, where nothing would arrive and the
/// read would wait on a person.
fn read_stdin() -> Result<String, String> {
    use std::io::IsTerminal as _;
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err("--from reads a JSON object on stdin, and none is piped in".to_string());
    }
    let mut text = String::new();
    stdin
        .lock()
        .take(STDIN_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("--from could not read stdin: {error}"))?;
    if text.len() as u64 > STDIN_BYTES {
        return Err(format!("--from reads at most {STDIN_BYTES} bytes of JSON on stdin"));
    }
    Ok(text)
}

enum Parsed {
    /// The report, and the transcript that has to record its compaction before it is sent.
    Report(Box<pane_request::Report>, Option<PathBuf>),
    Help,
}

fn parse(
    mut arguments: impl Iterator<Item = String>,
    environment: impl Fn(&str) -> Option<String>,
) -> Result<Parsed, String> {
    let mut report = pane_request::Report::default();
    let mut pane = None;
    let mut facts = HashMap::new();
    let mut compaction_in = None;
    while let Some(argument) = arguments.next() {
        let mut value = |flag: &str| arguments.next().ok_or(format!("{flag} needs a value"));
        match argument.as_str() {
            "--pane" => pane = Some(value("--pane")?),
            "--context-used" => {
                let given = value("--context-used")?;
                let percent = given.trim_end_matches('%');
                report.context_used = Some(
                    percent
                        .parse()
                        .map_err(|_| format!("--context-used {given} is not a number"))?,
                );
            }
            "--context-session" => report.context_session = Some(value("--context-session")?),
            "--model" => report.model = Some(value("--model")?),
            "--effort" => report.effort = Some(value("--effort")?),
            "--cost-usd" => {
                let given = value("--cost-usd")?;
                report.cost_usd =
                    Some(given.parse().map_err(|_| format!("--cost-usd {given} is not a number"))?);
            }
            "--subagent-started" | "--subagent-stopped" => {
                let started = report.subagent() == proto::SubagentChange::Started;
                match report.subagent() {
                    proto::SubagentChange::None => {}
                    _ if started == (argument == "--subagent-started") => {
                        return Err(format!("{argument} is given twice"));
                    }
                    _ => {
                        return Err(
                            "--subagent-started or --subagent-stopped, not both".to_string()
                        );
                    }
                }
                report.set_subagent(if argument == "--subagent-started" {
                    proto::SubagentChange::Started
                } else {
                    proto::SubagentChange::Stopped
                });
            }
            "--fact" => {
                let given = value("--fact")?;
                let (key, value) =
                    given.split_once('=').ok_or(format!("--fact {given} is not KEY=VALUE"))?;
                facts.insert(key.to_string(), value.to_string());
            }
            "--waiting" => report.waiting = Some(value("--waiting")?),
            "--until-message-from" => {
                let sender = value("--until-message-from")?;
                if sender.trim().is_empty() {
                    return Err("--until-message-from needs a pane".to_string());
                }
                report.waiting_until_message_from = Some(sender);
            }
            "--until-exits" => {
                let given = value("--until-exits")?;
                report.waiting_until_exits = Some(
                    given
                        .parse()
                        .ok()
                        .filter(|pid| *pid > 0)
                        .ok_or(format!("--until-exits {given} is not a process id"))?,
                );
            }
            "--clear" => report.clear = true,
            "--compacting" => report.compacting = true,
            "--session-name" => report.session_name = Some(value("--session-name")?),
            "--session-id" => report.session_id = Some(value("--session-id")?),
            "--session-source" => {
                report.set_context_change(from_source(&value("--session-source")?));
            }
            "--context-compacted" => report.set_context_change(proto::ContextChangeKind::Compacted),
            "--compacted-in" => {
                compaction_in = Some(PathBuf::from(value("--compacted-in")?));
                report.set_context_change(proto::ContextChangeKind::Compacted);
            }
            "--agent" => report.agent = value("--agent")?,
            "--state" => {
                let given = value("--state")?;
                report.set_state(match given.as_str() {
                    "working" => proto::AgentState::Working,
                    "blocked" => proto::AgentState::Blocked,
                    "idle" => proto::AgentState::Idle,
                    _ => return Err(format!("--state {given} is not working, blocked or idle")),
                });
            }
            "--help" | "-h" => return Ok(Parsed::Help),
            other => return Err(format!("{other} is not an option")),
        }
    }
    validate(&report)?;
    report.facts = facts;
    report.pane = pane.or_else(|| environment("MUSTER_PANE")).ok_or(
        "MUSTER_PANE is not set, so this is not running in a Muster pane; name one with --pane",
    )?;
    Ok(Parsed::Report(Box::new(report), compaction_in))
}

/// What a session starting from `source`, a hook's word for it, did to the agent's context.
fn from_source(source: &str) -> proto::ContextChangeKind {
    match source {
        "clear" => proto::ContextChangeKind::Cleared,
        "startup" => proto::ContextChangeKind::SessionStarted,
        _ => proto::ContextChangeKind::None,
    }
}

fn validate(report: &pane_request::Report) -> Result<(), String> {
    if report.waiting_until_exits.is_some() && report.waiting.as_deref().is_none_or(str::is_empty) {
        return Err("--until-exits needs --waiting, saying what the agent waits on".to_string());
    }
    if report.waiting_until_message_from.is_some() {
        if report.waiting.as_deref().is_none_or(str::is_empty) {
            return Err("--until-message-from needs --waiting".to_string());
        }
        if report.waiting_until_exits.is_some() {
            return Err("use --until-message-from or --until-exits, not both".to_string());
        }
    }
    if report.state.is_some() && report.agent.is_empty() {
        return Err("--state needs --agent, naming the agent that is reporting".to_string());
    }
    if report.session_name.is_some() && report.agent.is_empty() {
        return Err("--session-name needs --agent, naming the agent that is reporting".to_string());
    }
    if report.session_id.is_some() && report.agent.is_empty() {
        return Err("--session-id needs --agent, naming the agent that is reporting".to_string());
    }
    if report.compacting && report.agent.is_empty() {
        return Err("--compacting needs --agent, naming the agent that is reporting".to_string());
    }
    if report.context_change() != proto::ContextChangeKind::None && report.agent.is_empty() {
        return Err(
            "--context-compacted, --compacted-in and --session-source need --agent, naming the \
             agent that is reporting"
                .to_string(),
        );
    }
    Ok(())
}

/// Sends the report, giving up once `patience` has passed since it started.
///
/// On a thread of its own, so that one deadline covers dialing as well as every read and
/// write: std cannot time out a Unix socket's connect, and timeouts per step would add up. A
/// report still running when the deadline passes ends with the process.
fn send_within(
    socket: PathBuf,
    report: pane_request::Report,
    patience: Duration,
) -> Result<(), Unreported> {
    let (done, finished) = mpsc::channel();
    let race = Arc::new(Race::default());
    let racing = Arc::clone(&race);
    let dialed = socket.clone();
    std::thread::Builder::new()
        .name("report".to_string())
        .spawn(move || {
            // A handoff refuses what would change the pane until the new daemon serves the same
            // socket, which is moments later; a hook's report of a state must not be lost to it.
            let sent = loop {
                match send(&socket, report.clone(), &racing) {
                    Err(Unreported::NotReported(problem)) if problem.ends_with(HANDING_OVER) => {
                        racing.refused();
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    sent => break sent,
                }
            };
            let _ = done.send(sent);
        })
        .map_err(|error| {
            Unreported::NotReported(format!("could not start a thread to report from: {error}"))
        })?;
    finished.recv_timeout(patience).unwrap_or_else(|_| {
        Err(if race.give_up() {
            Unreported::NotReported(format!(
                "the daemon at {} did not take the report within {patience:?}, so nothing was \
                 reported; reporting again is safe",
                dialed.display()
            ))
        } else {
            Unreported::AnswerLost(format!(
                "the daemon took the report and did not answer within {patience:?}"
            ))
        })
    })
}

/// Between a report's deadline and its write, whichever claims the report first: a deadline that
/// gives up before the write starts stops it from starting, so "nothing was reported" stays
/// true, and one that comes after it says the report may have landed.
#[derive(Debug, Default)]
struct Race(AtomicU8);

impl Race {
    const UNCLAIMED: u8 = 0;
    const WRITING: u8 = 1;
    const GIVEN_UP: u8 = 2;

    /// Claims the report for writing; false once the deadline has given it up.
    fn write(&self) -> bool {
        self.claim(Self::UNCLAIMED, Self::WRITING)
    }

    /// The daemon refused what was written, so the report is unclaimed again for a retry.
    fn refused(&self) {
        self.claim(Self::WRITING, Self::UNCLAIMED);
    }

    /// Claims the report for the deadline; false when its write had already started.
    fn give_up(&self) -> bool {
        self.claim(Self::UNCLAIMED, Self::GIVEN_UP)
    }

    fn claim(&self, from: u8, to: u8) -> bool {
        self.0.compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }
}

/// Sends the report and reads its answer, writing it only if `race` lets it.
fn send(
    socket: &std::path::Path,
    report: pane_request::Report,
    race: &Race,
) -> Result<(), Unreported> {
    let unsent = Unreported::NotReported;
    let mut stream = UnixStream::connect(socket).map_err(|error| {
        unsent(format!("no daemon at {}, so nothing was reported: {error}", socket.display()))
    })?;
    let welcome = connection::open(&mut stream, ConnectionKind::Control, "muster-daemon report")
        .map_err(|error| unsent(error.to_string()))?;
    if report.waiting_until_message_from.is_some()
        && let Some(why) = proto::version::too_old(
            "the daemon",
            welcome.protocol.as_ref(),
            proto::version::WAIT_UNTIL_MESSAGE,
            "end a wait on a message",
            "declared",
        )
    {
        return Err(unsent(why));
    }
    let pane = report.pane.clone();
    let request = proto::Request {
        id: 1,
        service: Some(proto::request::Service::Pane(proto::PaneRequest {
            request: Some(pane_request::Request::Report(report)),
        })),
    };
    if !race.write() {
        return Err(unsent("given up before it was written".to_string()));
    }
    // A write that failed part of the way may still have delivered the report.
    connection::send(&mut stream, &request)
        .map_err(|error| Unreported::AnswerLost(format!("the report was not all sent: {error}")))?;
    // Not subscribed, so the one message coming is the answer.
    let answer = match connection::receive::<proto::ControlMessage>(&mut stream) {
        Ok(Some(proto::ControlMessage {
            message: Some(proto::control_message::Message::Answer(answer)),
        })) => answer,
        Ok(_) => {
            return Err(Unreported::AnswerLost(
                "the daemon took the report and hung up without answering".to_string(),
            ));
        }
        Err(error) => {
            return Err(Unreported::AnswerLost(format!(
                "the daemon took the report and did not answer: {error}"
            )));
        }
    };
    match answer.outcome() {
        proto::Outcome::Done | proto::Outcome::AlreadySo => Ok(()),
        proto::Outcome::NotThere => Err(unsent(format!(
            "the daemon has no pane {pane}; name one by the id, callsign or name `muster window` \
             lists it under"
        ))),
        _ => Err(unsent(format!("the daemon refused the report: {}", answer.reason))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sender_condition_needs_a_wait_and_excludes_a_process_condition() {
        let parse =
            |args: &[&str]| parse(args.iter().map(|s| (*s).to_string()), |_| Some("p1".into()));
        let Ok(Parsed::Report(report, _)) =
            parse(&["--waiting", "review", "--until-message-from", "otter"])
        else {
            panic!("valid sender wait refused")
        };
        assert_eq!(report.waiting_until_message_from.as_deref(), Some("otter"));
        for args in [
            vec!["--until-message-from", "otter"],
            vec!["--waiting", "", "--until-message-from", "otter"],
            vec!["--waiting", "review", "--until-message-from", ""],
            vec!["--waiting", "review", "--until-message-from", "otter", "--until-exits", "42"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }

    fn filled(arguments: &[&str], stdin: &str) -> Result<Vec<String>, String> {
        let arguments = arguments.iter().map(|argument| (*argument).to_string()).collect();
        filled_from(arguments, || Ok(stdin.to_string())).map(|filled| filled.arguments)
    }

    /// Claude Code's statusline JSON, cut down to what its statusline reports.
    const STATUS: &str = r#"{"model":{"display_name":"Opus"},"effort":{"level":"high"},"context_window":{"used_percentage":42.5},"cost":{"total_cost_usd":1.5},"session_name":null}"#;

    #[test]
    fn from_fills_each_flag_out_of_the_json_on_stdin() {
        let arguments = [
            "--agent",
            "claude",
            "--session-name",
            "",
            "--from",
            "context-used=/context_window/used_percentage",
            "--from",
            "model=/model/display_name",
            "--from",
            "effort=/effort/level",
            "--from",
            "cost-usd=/cost/total_cost_usd",
            "--from",
            "session-name=/session_name",
            "--from",
            "session-id=/session_id",
        ];
        assert_eq!(
            filled(&arguments, STATUS).unwrap(),
            [
                "--agent",
                "claude",
                "--session-name",
                "",
                "--context-used",
                "42.5",
                "--model",
                "Opus",
                "--effort",
                "high",
                "--cost-usd",
                "1.5",
            ],
            "a null and a missing value each leave their flag out"
        );
        let named = STATUS.replace(r#""session_name":null"#, r#""session_name":"🤖 A""#);
        let filled =
            filled(&["--session-name", "", "--from", "session-name=/session_name"], &named);
        let report = parsed(
            &filled
                .unwrap()
                .iter()
                .map(String::as_str)
                .chain(["--agent", "claude"])
                .collect::<Vec<_>>(),
            Some("p1"),
        )
        .unwrap();
        assert_eq!(
            report.session_name.as_deref(),
            Some("🤖 A"),
            "a later flag replaces an earlier"
        );
    }

    #[test]
    fn from_reads_stdin_only_when_asked_and_refuses_what_it_cannot_fill() {
        let untouched = filled_from(vec!["--clear".to_string()], || panic!("stdin was read"));
        let untouched = untouched.unwrap();
        assert_eq!(untouched.arguments, ["--clear"]);
        assert!(!untouched.came_up_empty, "no --from, so nothing came up empty");
        let refused = |arguments: &[&str], stdin: &str| filled(arguments, stdin).unwrap_err();
        assert!(refused(&["--from", "model=/model"], "not json").contains("not one"));
        assert!(refused(&["--from", "clear=/x"], "{}").contains("fills only"));
        assert!(refused(&["--from", "model"], "{}").contains("FLAG=POINTER"));
        assert!(refused(&["--from"], "{}").contains("needs a value"));
        assert!(refused(&["--from", "model=/model"], STATUS).contains("not text or a number"));
    }

    fn parsed(arguments: &[&str], pane: Option<&str>) -> Result<pane_request::Report, String> {
        let arguments = arguments.iter().map(|argument| (*argument).to_string());
        match parse(arguments, |name| (name == "MUSTER_PANE").then(|| pane.map(str::to_string))?)? {
            Parsed::Report(report, _) => Ok(*report),
            Parsed::Help => Err("help".to_string()),
        }
    }

    /// A statusline names the session its context belongs to, which is not the pane's session id.
    #[test]
    fn a_statusline_names_the_session_its_context_belongs_to() {
        let arguments = ["--agent", "claude", "--from", "context-session=/session_id"];
        let given = filled(&arguments, r#"{"session_id":"0199-c"}"#).unwrap();
        let report = parsed(&given.iter().map(String::as_str).collect::<Vec<_>>(), Some("p1"));
        let report = report.unwrap();
        assert_eq!(report.context_session.as_deref(), Some("0199-c"));
        assert_eq!(report.session_id, None);
    }

    #[test]
    fn a_sessions_source_says_what_it_did_to_the_context() {
        let source = |source: &str| {
            let arguments = ["--agent", "codex", "--session-source", source];
            parsed(&arguments, Some("p1")).unwrap().context_change()
        };
        assert_eq!(source("clear"), proto::ContextChangeKind::Cleared);
        assert_eq!(source("startup"), proto::ContextChangeKind::SessionStarted);
        assert_eq!(source("resume"), proto::ContextChangeKind::None);
        assert_eq!(source("compact"), proto::ContextChangeKind::None);
    }

    #[test]
    fn a_change_of_context_needs_its_agent() {
        assert!(parsed(&["--context-compacted"], Some("p1")).is_err());
        assert!(parsed(&["--session-source", "clear"], Some("p1")).is_err());
    }

    /// A sub-agent's compaction reads as its parent's unless the transcript says otherwise, so a
    /// hook's input with no transcript to ask reports nothing, rather than a compaction unchecked.
    #[test]
    fn a_compaction_with_no_transcript_to_confirm_it_says_nothing() {
        let arguments = ["--agent", "claude", "--from", "compacted-in=/transcript_path"];
        let Filled { arguments: given, came_up_empty } =
            filled_from(arguments.map(String::from).to_vec(), || Ok("{}".to_string())).unwrap();
        assert!(came_up_empty);
        let report = parsed(&given.iter().map(String::as_str).collect::<Vec<_>>(), Some("p1"));
        assert!(says_nothing(&report.unwrap()));
    }

    /// A hook hands its session's id in the JSON on its standard input, however the session
    /// started, which `--from` reads so the hook needs no JSON tool of its own; input with no id
    /// leaves a report that says nothing, which is not sent.
    #[test]
    fn a_session_id_is_read_from_a_hooks_input() {
        let arguments = ["--agent", "claude", "--from", "session-id=/session_id"];
        let input =
            r#"{"session_id":"0199-a b","hook_event_name":"SessionStart","source":"resume"}"#;
        let given = filled(&arguments, input).unwrap();
        let report = parsed(&given.iter().map(String::as_str).collect::<Vec<_>>(), Some("p1"));
        assert_eq!(report.unwrap().session_id.as_deref(), Some("0199-a b"));

        let empty = |arguments: &[&str]| {
            let arguments = arguments.iter().map(|argument| (*argument).to_string()).collect();
            filled_from(arguments, || Ok(r#"{"source":"startup"}"#.to_string())).unwrap()
        };
        let none = empty(&arguments);
        assert!(none.came_up_empty, "an input with no id fills nothing");
        let report =
            parsed(&none.arguments.iter().map(String::as_str).collect::<Vec<_>>(), Some("p1"));
        assert!(says_nothing(&report.unwrap()), "and the report has nothing to tell");
        assert!(!says_nothing(&parsed(&["--clear"], Some("p1")).unwrap()), "--clear is something");
    }

    #[test]
    fn a_report_names_its_own_pane_and_what_it_was_given() {
        let report = parsed(
            &["--context-used", "42%", "--model", "Opus", "--fact", "branch=main", "--clear"],
            Some("p1"),
        )
        .unwrap();
        assert_eq!(report.pane, "p1");
        assert_eq!(report.context_used, Some(42.0));
        assert_eq!(report.model.as_deref(), Some("Opus"));
        assert_eq!(report.facts.get("branch").map(String::as_str), Some("main"));
        assert!(report.clear);
        let waiting = parsed(&["--waiting", "the full gate"], Some("p1")).unwrap();
        assert_eq!(waiting.waiting.as_deref(), Some("the full gate"));
        let ended = parsed(&["--waiting", ""], Some("p1")).unwrap();
        assert_eq!(ended.waiting.as_deref(), Some(""), "an empty wait is sent, to clear one");
        let on_a_job = parsed(&["--waiting", "the gate", "--until-exits", "4242"], Some("p1"));
        assert_eq!(on_a_job.unwrap().waiting_until_exits, Some(4242));
        let started = parsed(&["--subagent-started", "--pane", "p2"], Some("p1")).unwrap();
        assert_eq!(
            (started.pane.as_str(), started.subagent()),
            ("p2", proto::SubagentChange::Started)
        );
    }

    #[test]
    fn a_state_is_reported_with_the_agent_reporting_it() {
        let report = parsed(&["--agent", "claude", "--state", "blocked"], Some("p1")).unwrap();
        assert_eq!((report.agent.as_str(), report.state()), ("claude", proto::AgentState::Blocked));
        assert!(parsed(&["--state", "idle"], Some("p1")).unwrap_err().contains("--agent"));
        assert!(
            parsed(&["--agent", "x", "--state", "done"], Some("p1")).unwrap_err().contains("done")
        );
    }

    #[test]
    fn a_session_name_is_reported_with_the_agent_reporting_it() {
        let named = parsed(&["--agent", "claude", "--session-name", "🤖 A"], Some("p1")).unwrap();
        assert_eq!(named.session_name.as_deref(), Some("🤖 A"));
        let unnamed = parsed(&["--agent", "claude", "--session-name", ""], Some("p1")).unwrap();
        assert_eq!(unnamed.session_name.as_deref(), Some(""), "no name is said, not left out");
        let alone = parsed(&["--session-name", "A"], Some("p1"));
        assert!(alone.unwrap_err().contains("--agent"));
    }

    #[test]
    fn a_session_id_is_reported_with_the_agent_reporting_it() {
        let given = parsed(&["--agent", "codex", "--session-id", "019a-b"], Some("p1")).unwrap();
        assert_eq!(given.session_id.as_deref(), Some("019a-b"));
        assert!(parsed(&["--session-id", "019a-b"], Some("p1")).unwrap_err().contains("--agent"));
    }

    #[test]
    fn a_report_outside_a_pane_or_with_a_bad_value_says_why() {
        assert!(parsed(&["--model", "x"], None).unwrap_err().contains("MUSTER_PANE"));
        assert!(parsed(&["--cost-usd", "lots"], Some("p1")).unwrap_err().contains("--cost-usd"));
        assert!(parsed(&["--fact", "novalue"], Some("p1")).unwrap_err().contains("KEY=VALUE"));
        assert!(parsed(&["--model"], Some("p1")).unwrap_err().contains("needs a value"));
        let alone = parsed(&["--until-exits", "4242"], Some("p1"));
        assert!(alone.unwrap_err().contains("needs --waiting"));
        for pid in ["0", "-1", "job"] {
            let bad = parsed(&["--waiting", "x", "--until-exits", pid], Some("p1"));
            assert!(bad.unwrap_err().contains("not a process id"), "{pid}");
        }
        let twice = parsed(&["--subagent-stopped", "--subagent-stopped"], Some("p1"));
        assert!(twice.unwrap_err().contains("given twice"));
        let both = parsed(&["--subagent-started", "--subagent-stopped"], Some("p1"));
        assert!(
            both.unwrap_err().contains("not both"),
            "a sub-agent cannot start and stop at once"
        );
    }

    /// A deadline that gives a report up before its write starts stops the write, so "nothing was
    /// reported" stays true; one that comes after the write started cannot give it up.
    #[test]
    fn a_report_is_either_given_up_or_written_never_both() {
        let given_up = Race::default();
        assert!(given_up.give_up());
        assert!(!given_up.write(), "written after the deadline said nothing was reported");

        let written = Race::default();
        assert!(written.write());
        assert!(!written.give_up(), "given up after the write started");

        let retried = Race::default();
        assert!(retried.write());
        retried.refused();
        assert!(retried.give_up(), "a refused write leaves the report for the deadline");
    }

    /// A daemon that answers each step just inside a per-step timeout would hold a hook up for
    /// the sum of them: the whole report shares one deadline.
    #[test]
    fn a_report_gives_up_once_its_one_deadline_passes() {
        let socket =
            std::env::temp_dir().join(format!("muster-report-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        // Each step inside the patience, both together well past it, and wide apart enough that
        // a loaded machine cannot blur the two outcomes.
        let step = Duration::from_secs(1);
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = connection::receive::<proto::Hello>(&mut stream);
            std::thread::sleep(step);
            let welcome = proto::HelloAnswer {
                answer: Some(proto::hello_answer::Answer::Welcome(proto::Welcome::default())),
            };
            let _ = connection::send(&mut stream, &welcome);
            let Ok(Some(request)) = connection::receive::<proto::Request>(&mut stream) else {
                return;
            };
            std::thread::sleep(step);
            let answer = proto::Answer {
                id: request.id,
                outcome: proto::Outcome::Done.into(),
                ..proto::Answer::default()
            };
            let _ = connection::send(
                &mut stream,
                &proto::ControlMessage {
                    message: Some(proto::control_message::Message::Answer(answer)),
                },
            );
        });

        let started = std::time::Instant::now();
        let sent = send_within(socket.clone(), parsed(&[], Some("p1")).unwrap(), step + step / 5);
        let took = started.elapsed();
        let _ = std::fs::remove_file(&socket);
        // The report was written before the deadline passed, so it may have landed.
        assert!(
            matches!(&sent, Err(Unreported::AnswerLost(why)) if why.contains("did not answer")),
            "{sent:?}"
        );
        assert!(took < step * 9 / 5, "the report waited {took:?}");
    }
}
