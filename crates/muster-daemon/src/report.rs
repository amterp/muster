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
use std::time::Duration;

use muster_daemon_proto::{self as proto, ConnectionKind, connection, pane_request};

/// How long a report waits for its daemon. A hook that runs on every sub-agent and a statusline
/// that runs on every message must not hold their harness up.
const PATIENCE: Duration = Duration::from_secs(2);

const USAGE: &str = "usage: muster-daemon report [--pane NAME] [--context-used PERCENT] \
    [--model NAME] [--cost-usd DOLLARS] [--subagent-started | --subagent-stopped] \
    [--fact KEY=VALUE]... [--clear]\n\n\
    Tells the daemon that owns this pane what the agent in it says about itself. The pane is \
    $MUSTER_PANE unless --pane names another, and the daemon is the one at \
    $MUSTER_DAEMON_SOCKET. A fact with an empty value is removed; --clear forgets everything \
    reported before, and applies first.";

pub(crate) fn run(arguments: impl Iterator<Item = String>) -> ExitCode {
    let report = match parse(arguments, |name| std::env::var(name).ok()) {
        Ok(Parsed::Report(report)) => report,
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
    match send(&socket, report) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("muster-daemon report: {problem}");
            ExitCode::FAILURE
        }
    }
}

enum Parsed {
    Report(pane_request::Report),
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
            "--subagent-started" => report.set_subagent(proto::SubagentChange::Started),
            "--subagent-stopped" => report.set_subagent(proto::SubagentChange::Stopped),
            "--fact" => {
                let given = value("--fact")?;
                let (key, value) =
                    given.split_once('=').ok_or(format!("--fact {given} is not KEY=VALUE"))?;
                facts.insert(key.to_string(), value.to_string());
            }
            "--clear" => report.clear = true,
            "--help" | "-h" => return Ok(Parsed::Help),
            other => return Err(format!("{other} is not an option")),
        }
    }
    report.facts = facts;
    report.pane = pane.or_else(|| environment("MUSTER_PANE")).ok_or(
        "MUSTER_PANE is not set, so this is not running in a Muster pane; name one with --pane",
    )?;
    Ok(Parsed::Report(report))
}

fn send(socket: &std::path::Path, report: pane_request::Report) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|error| format!("no daemon at {}: {error}", socket.display()))?;
    stream.set_read_timeout(Some(PATIENCE)).map_err(|error| error.to_string())?;
    stream.set_write_timeout(Some(PATIENCE)).map_err(|error| error.to_string())?;
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
            Parsed::Report(report) => Ok(report),
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
        let started = parsed(&["--subagent-started", "--pane", "p2"], Some("p1")).unwrap();
        assert_eq!(
            (started.pane.as_str(), started.subagent()),
            ("p2", proto::SubagentChange::Started)
        );
    }

    #[test]
    fn a_report_outside_a_pane_or_with_a_bad_value_says_why() {
        assert!(parsed(&["--model", "x"], None).unwrap_err().contains("MUSTER_PANE"));
        assert!(parsed(&["--cost-usd", "lots"], Some("p1")).unwrap_err().contains("--cost-usd"));
        assert!(parsed(&["--fact", "novalue"], Some("p1")).unwrap_err().contains("KEY=VALUE"));
        assert!(parsed(&["--model"], Some("p1")).unwrap_err().contains("needs a value"));
    }
}
