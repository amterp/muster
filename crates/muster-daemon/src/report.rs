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
use std::sync::mpsc;
use std::time::Duration;

use muster_daemon_proto::{self as proto, ConnectionKind, connection, pane_request};

use crate::session::HANDING_OVER;

/// The most JSON `--from` reads from stdin. A statusline's or a hook's input is a few kilobytes.
const STDIN_BYTES: u64 = 1 << 20;

/// The flags `--from` may fill: each one that takes one value.
const FROM_FLAGS: [&str; 8] = [
    "context-used",
    "model",
    "cost-usd",
    "waiting",
    "session-name",
    "session-id",
    "agent",
    "state",
];

/// How long a report waits for its daemon, from dialing to the answer. A hook that runs on every
/// sub-agent and a statusline that runs on every message must not hold their harness up.
const PATIENCE: Duration = Duration::from_secs(2);

const USAGE: &str = "usage: muster-daemon report [--pane NAME] [--context-used PERCENT] \
    [--model NAME] [--cost-usd DOLLARS] [--subagent-started | --subagent-stopped] \
    [--fact KEY=VALUE]... [--waiting TEXT] [--clear] \
    [--agent NAME [--state working|blocked|idle] [--session-name NAME] \
    [--session-id ID]] [--from FLAG=POINTER]...\n\n\
    Tells the daemon that owns this pane what the agent in it says about itself. The pane is \
    $MUSTER_PANE unless --pane names another, and the daemon is the one at \
    $MUSTER_DAEMON_SOCKET. An empty model or fact value removes it; --clear forgets everything \
    reported before, and applies first. --waiting says what the agent ended its turn to wait \
    on, work it started itself; it lasts until a later turn ends without saying it again, and \
    an empty one clears it. Say it in each turn that ends to wait: work that never wakes the \
    agent leaves the pane waiting until the next turn. --state is the agent's own word on what \
    it is doing, \
    which outranks what detection reads off its screen while fresh; --agent names the agent, \
    as its detection manifest does (claude), and the state counts only while that agent is \
    the pane's. --session-name is what the agent's harness calls its session, empty for no \
    name: the pane takes a name the session is given once it has started, and gives the \
    session the pane's own name otherwise. It needs --agent, as --state does. --session-id is \
    the harness's id for its session, which the daemon names it by to reach it through the \
    harness's own command, where its manifest says how; empty forgets it. It needs --agent too. \
    --from fills a flag from the JSON object on stdin, which a harness hands its hooks and \
    statusline: --from context-used=/context_window/used_percentage reads that JSON pointer, a \
    number or text, as --context-used's value. A value missing or null leaves the flag out, and \
    a flag given later replaces one given before, so --session-name '' before a --from \
    session-name says no name when the JSON has none, and --from session-id=/session_id reads \
    the id a hook is handed. Stdin is read only for --from, and never from a terminal. A report \
    whose every --from found nothing, and that says nothing else, is not sent.";

pub(crate) fn run(arguments: impl Iterator<Item = String>) -> ExitCode {
    let Filled { arguments, came_up_empty } = match filled_from(arguments.collect(), read_stdin) {
        Ok(filled) => filled,
        Err(problem) => {
            eprintln!("muster-daemon report: {problem}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let report = match parse(arguments.into_iter(), |name| std::env::var(name).ok()) {
        Ok(Parsed::Report(report)) => *report,
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
    let Some(socket) = std::env::var_os("MUSTER_DAEMON_SOCKET").map(PathBuf::from) else {
        eprintln!(
            "muster-daemon report: MUSTER_DAEMON_SOCKET is not set, so this is not running in a \
             pane of a Muster daemon and there is nobody to tell"
        );
        return ExitCode::FAILURE;
    };
    match send_within(socket, report, PATIENCE) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("muster-daemon report: {problem}");
            ExitCode::FAILURE
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
    Report(Box<pane_request::Report>),
    Help,
}

fn parse(
    mut arguments: impl Iterator<Item = String>,
    environment: impl Fn(&str) -> Option<String>,
) -> Result<Parsed, String> {
    let mut report = pane_request::Report::default();
    let mut pane = None;
    let mut facts = HashMap::new();
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
            "--model" => report.model = Some(value("--model")?),
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
            "--clear" => report.clear = true,
            "--session-name" => report.session_name = Some(value("--session-name")?),
            "--session-id" => report.session_id = Some(value("--session-id")?),
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
    if report.state.is_some() && report.agent.is_empty() {
        return Err("--state needs --agent, naming the agent that is reporting".to_string());
    }
    if report.session_name.is_some() && report.agent.is_empty() {
        return Err("--session-name needs --agent, naming the agent that is reporting".to_string());
    }
    if report.session_id.is_some() && report.agent.is_empty() {
        return Err("--session-id needs --agent, naming the agent that is reporting".to_string());
    }
    report.facts = facts;
    report.pane = pane.or_else(|| environment("MUSTER_PANE")).ok_or(
        "MUSTER_PANE is not set, so this is not running in a Muster pane; name one with --pane",
    )?;
    Ok(Parsed::Report(Box::new(report)))
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
) -> Result<(), String> {
    let (done, finished) = mpsc::channel();
    std::thread::Builder::new()
        .name("report".to_string())
        .spawn(move || {
            // A handoff refuses what would change the pane until the new daemon serves the same
            // socket, which is moments later; a hook's report of a state must not be lost to it.
            let sent = loop {
                match send(&socket, report.clone()) {
                    Err(problem) if problem.ends_with(HANDING_OVER) => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    sent => break sent,
                }
            };
            let _ = done.send(sent);
        })
        .map_err(|error| format!("could not start a thread to report from: {error}"))?;
    finished
        .recv_timeout(patience)
        .unwrap_or_else(|_| Err(format!("the daemon did not answer within {patience:?}")))
}

fn send(socket: &std::path::Path, report: pane_request::Report) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|error| format!("no daemon at {}: {error}", socket.display()))?;
    connection::open(&mut stream, ConnectionKind::Control, "muster-daemon report")
        .map_err(|error| error.to_string())?;
    let pane = report.pane.clone();
    let request = proto::Request {
        id: 1,
        service: Some(proto::request::Service::Pane(proto::PaneRequest {
            request: Some(pane_request::Request::Report(report)),
        })),
    };
    connection::send(&mut stream, &request).map_err(|error| error.to_string())?;
    // Not subscribed, so the one message coming is the answer.
    let answer = match connection::receive::<proto::ControlMessage>(&mut stream) {
        Ok(Some(proto::ControlMessage {
            message: Some(proto::control_message::Message::Answer(answer)),
        })) => answer,
        Ok(_) => return Err("the daemon hung up without answering".to_string()),
        Err(error) => return Err(format!("the daemon did not answer: {error}")),
    };
    match answer.outcome() {
        proto::Outcome::Done | proto::Outcome::AlreadySo => Ok(()),
        proto::Outcome::NotThere => Err(format!("the daemon has no pane {pane}")),
        _ => Err(format!("the daemon refused the report: {}", answer.reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(arguments: &[&str], stdin: &str) -> Result<Vec<String>, String> {
        let arguments = arguments.iter().map(|argument| (*argument).to_string()).collect();
        filled_from(arguments, || Ok(stdin.to_string())).map(|filled| filled.arguments)
    }

    /// Claude Code's statusline JSON, cut down to what its statusline reports.
    const STATUS: &str = r#"{"model":{"display_name":"Opus"},"context_window":{"used_percentage":42.5},"cost":{"total_cost_usd":1.5},"session_name":null}"#;

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
            Parsed::Report(report) => Ok(*report),
            Parsed::Help => Err("help".to_string()),
        }
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
        let twice = parsed(&["--subagent-stopped", "--subagent-stopped"], Some("p1"));
        assert!(twice.unwrap_err().contains("given twice"));
        let both = parsed(&["--subagent-started", "--subagent-stopped"], Some("p1"));
        assert!(
            both.unwrap_err().contains("not both"),
            "a sub-agent cannot start and stop at once"
        );
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
        assert!(sent.unwrap_err().contains("did not answer"));
        assert!(took < step * 9 / 5, "the report waited {took:?}");
    }
}
