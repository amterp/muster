//! `muster-daemon report`: what the agent in a pane says about itself, told to the daemon that
//! owns the pane.
//!
//! Run from inside a pane - by a harness's hooks and statusline - it finds its pane and its
//! daemon in the environment the daemon gave the pane (`MUSTER_PANE`, `MUSTER_DAEMON_SOCKET`),
//! so it needs no window and works on a machine where the daemon is the only piece of Muster
//! installed. It is quick and quiet: one request, a short timeout, and nothing printed unless
//! the daemon did not take the report.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use muster_daemon_proto::{self as proto, ConnectionKind, connection, pane_request};

use crate::session::HANDING_OVER;

/// How long a report waits for its daemon, from dialing to the answer. A hook that runs on every
/// sub-agent and a statusline that runs on every message must not hold their harness up.
const PATIENCE: Duration = Duration::from_secs(2);

const USAGE: &str = "usage: muster-daemon report [--pane NAME] [--context-used PERCENT] \
    [--model NAME] [--cost-usd DOLLARS] [--subagent-started | --subagent-stopped] \
    [--fact KEY=VALUE]... [--waiting TEXT] [--clear] \
    [--agent NAME [--state working|blocked|idle] [--session-name NAME]]\n\n\
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
    session the pane's own name otherwise. It needs --agent, as --state does.";

pub(crate) fn run(arguments: impl Iterator<Item = String>) -> ExitCode {
    let report = match parse(arguments, |name| std::env::var(name).ok()) {
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

    fn parsed(arguments: &[&str], pane: Option<&str>) -> Result<pane_request::Report, String> {
        let arguments = arguments.iter().map(|argument| (*argument).to_string());
        match parse(arguments, |name| (name == "MUSTER_PANE").then(|| pane.map(str::to_string))?)? {
            Parsed::Report(report) => Ok(*report),
            Parsed::Help => Err("help".to_string()),
        }
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
