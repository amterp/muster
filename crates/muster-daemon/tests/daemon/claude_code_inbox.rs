//! What Claude Code does with a message that reaches its inbox socket from a process that is not
//! one of its own children - which is what muster-daemon is to every Claude session it wakes.
//! Claude Code's documentation says what happens to a message from another Claude session and to
//! one from the session's own child, and leaves this case open, so the answer is recorded here
//! from the Claude Code installed on this machine: delivered, held for a person's approval, or
//! refused, per permission mode and per whether the sender presents the session's token.
//!
//! `docs/observations/claude-code-<version>.md` reads the transcripts this writes into
//! `corpus/claude-code-<version>/`, and the inbox adapter's default rests on them. Run with
//! `MUSTER_RECORD_CLAUDE_INBOX=1` it records; run without, it holds the installed Claude Code to
//! the newest recording, so an update that changes the answer fails `./dev --claude-code` rather
//! than silently stranding the agents the daemon wakes.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::claude_code_live::{how_to_run, quoted, until_ready};
use crate::support::*;
use muster_harness::Input;

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus");

/// Long enough for a delivered message to start a turn and a held one to raise its dialog: both
/// showed within five seconds in every recorded run, so twelve times that says neither is coming.
const VERDICT: Duration = Duration::from_mins(1);

/// A bare connection is the daemon's liveness probe. Nothing it could show had shown within one
/// second in the recorded runs; three leaves room for a loaded machine.
const BARE: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    bypass: bool,
    token: bool,
    accept: bool,
}

const CASES: [Case; 5] = [
    Case { name: "default-none", bypass: false, token: false, accept: false },
    Case { name: "default-token", bypass: false, token: true, accept: false },
    Case { name: "bypass-none", bypass: true, token: false, accept: false },
    Case { name: "bypass-token", bypass: true, token: true, accept: false },
    Case { name: "bypass-none-accept", bypass: true, token: false, accept: true },
];

impl Case {
    fn describe(self) -> String {
        format!(
            "{} mode, {}{}",
            if self.bypass { "bypassPermissions" } else { "default" },
            if self.token { "auth line with the session's token" } else { "no auth line" },
            if self.accept { ", crossSessionInbound \"accept\" passed with --settings" } else { "" },
        )
    }

    /// The settings passed with `--settings`: a hook that tells the test where the session's
    /// inbox is, since only the session's own children are told, and nothing else of anyone's.
    fn settings(self) -> serde_json::Value {
        let hook = "printf '%s\\n%s\\n' \"$CLAUDE_CODE_MESSAGING_SOCKET\" \
                    \"$CLAUDE_CODE_MESSAGING_TOKEN\" > inbox.txt";
        let mut settings = serde_json::json!({
            "hooks": { "SessionStart": [{ "hooks": [{ "type": "command", "command": hook }] }] },
            "skipDangerousModePermissionPrompt": true,
        });
        if self.accept {
            settings["crossSessionInbound"] = "accept".into();
        }
        settings
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Delivered,
    Held,
    Refused,
}

impl Outcome {
    fn name(self) -> &'static str {
        match self {
            Outcome::Delivered => "delivered",
            Outcome::Held => "held",
            Outcome::Refused => "refused",
        }
    }

    /// What Claude Code 2.1.283 shows for each: the held one as a notice and an approval
    /// dialog, the delivered one as the start of a turn. Held is asked first because its dialog
    /// quotes the same "Another Claude session sent a message" a delivered one opens with.
    fn shown_in(screen: &str) -> Option<Outcome> {
        if screen.contains("Held peer message") || screen.contains("Held message from") {
            Some(Outcome::Held)
        } else if screen.contains("Another Claude session sent a message") {
            Some(Outcome::Delivered)
        } else {
            None
        }
    }
}

struct Seen {
    case: Case,
    bare_screen: String,
    sent: Vec<String>,
    received: Vec<u8>,
    outcome: Outcome,
    screen: String,
}

fn command_output(program: &str, arguments: &[&str]) -> String {
    Command::new(program)
        .args(arguments)
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Where the session's `SessionStart` hook said its inbox is.
fn inbox_of(project: &Path) -> (PathBuf, String) {
    let written = written(&project.join("inbox.txt"));
    let mut lines = written.lines();
    let socket = PathBuf::from(lines.next().unwrap_or_default());
    let token = lines.next().unwrap_or_default().to_string();
    assert!(
        socket.is_absolute() && !token.is_empty(),
        "Claude Code gave its SessionStart hook no inbox: {written:?}. Impact: nothing can be \
         sent to it, so this records nothing. Check that this Claude Code has cross-session \
         messaging (v2.1.224 or later) and that /status shows a Peer address."
    );
    (socket, token)
}

fn send(socket: &Path, lines: &[String]) -> Vec<u8> {
    let mut connection = UnixStream::connect(socket)
        .unwrap_or_else(|error| panic!("{}: {error}", socket.display()));
    let mut text = lines.join("\n");
    text.push('\n');
    connection.write_all(text.as_bytes()).expect("the inbox takes the lines");
    // Whatever Claude Code says back, if anything: none of the recorded runs said anything.
    connection.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut received = Vec::new();
    let _ = connection.read_to_end(&mut received);
    received
}

fn observe(control: &mut Control, pane: &str, case: Case, socket: &Path, token: &str) -> Seen {
    drop(UnixStream::connect(socket).expect("the inbox accepts a connection"));
    // Proving a negative: nothing to wait on but time (see BARE).
    std::thread::sleep(BARE);
    let bare_screen = read_text(control, pane, 0, 0).text;

    let mut lines = Vec::new();
    if case.token {
        lines.push(serde_json::json!({ "type": "auth", "token": token }).to_string());
    }
    let body = format!("Observation {}: reply with only the word ACKNOWLEDGED.", case.name);
    let message = serde_json::json!({
        "type": "user", "message": { "role": "user", "content": body }
    });
    lines.push(message.to_string());
    let received = send(socket, &lines);

    let deadline = Instant::now() + VERDICT;
    let (outcome, screen) = loop {
        let screen = read_text(control, pane, 0, 0).text;
        if let Some(outcome) = Outcome::shown_in(&screen) {
            // Let a delivered message's turn finish, so the transcript shows the answer.
            std::thread::sleep(Duration::from_secs(5));
            break (outcome, read_text(control, pane, 0, 0).text);
        }
        if Instant::now() >= deadline {
            break (Outcome::Refused, screen);
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let sent = lines.iter().map(|line| line.replace(token, "<token>")).collect();
    Seen { case, bare_screen, sent, received, outcome, screen }
}

fn recording_directory(version: &str) -> PathBuf {
    Path::new(CORPUS).join(format!("claude-code-{version}"))
}

/// The recording to hold this Claude Code to: its own version's, else the newest there is.
fn newest_recording() -> Option<(String, serde_json::Value)> {
    let mut recorded: Vec<(Vec<u32>, PathBuf)> = std::fs::read_dir(CORPUS)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = name.strip_prefix("claude-code-")?.to_string();
            let parts = version.split('.').map(|part| part.parse().unwrap_or(0)).collect();
            let file = entry.path().join("inbox.json");
            file.exists().then_some((parts, file))
        })
        .collect();
    recorded.sort();
    let (_, file) = recorded.pop()?;
    let text = std::fs::read_to_string(&file).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some((json["claude_code"].as_str()?.to_string(), json))
}

fn record(version: &str, seen: &[Seen]) {
    let directory = recording_directory(version);
    std::fs::create_dir_all(&directory).unwrap();
    let when = command_output("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]);
    let os = format!(
        "macOS {} {}",
        command_output("sw_vers", &["-productVersion"]),
        command_output("uname", &["-m"])
    );
    let mut cases = Vec::new();
    for seen in seen {
        let file = format!("inbox-{}.txt", seen.case.name);
        let transcript = format!(
            "# what Claude Code {version} does with a message on its inbox socket from a\n\
             # process that is not its child: {}\n\
             #\n\
             # recorded {when} on {os}\n\
             # by crates/muster-daemon/tests/daemon/claude_code_inbox.rs\n\
             # (MUSTER_RECORD_CLAUDE_INBOX=1 ./dev --claude-code);\n\
             # read in docs/observations/claude-code-{version}.md\n\
             #\n\
             # The sender is the test process; Claude Code runs in a pane of a daemon the test\n\
             # started, so the sender is neither the session nor any child of it.\n\
             \n\
             outcome: {}\n\
             \n\
             screen three seconds after a connection that sent nothing:\n{}\n\
             \n\
             sent, one line each:\n{}\n\
             \n\
             received back before the connection closed or three seconds passed: {:?}\n\
             \n\
             screen once the outcome showed:\n{}\n",
            seen.case.describe(),
            seen.outcome.name(),
            indent(&seen.bare_screen),
            indent(&seen.sent.join("\n")),
            String::from_utf8_lossy(&seen.received),
            indent(&seen.screen),
        );
        std::fs::write(directory.join(&file), transcript).unwrap();
        cases.push(serde_json::json!({
            "case": seen.case.name,
            "describe": seen.case.describe(),
            "outcome": seen.outcome.name(),
            "transcript": file,
        }));
    }
    let summary = serde_json::json!({
        "claude_code": version, "recorded": when, "os": os, "cases": cases,
    });
    let mut text = serde_json::to_string_pretty(&summary).unwrap();
    text.push('\n');
    std::fs::write(directory.join("inbox.json"), text).unwrap();
    eprintln!("claude-code: recorded into {}", directory.display());
}

fn indent(text: &str) -> String {
    text.lines().map(|line| format!("    {line}")).collect::<Vec<_>>().join("\n")
}

#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn claude_code_inbox_delivers_holds_or_refuses_as_recorded() {
    if std::env::var_os("MUSTER_CLAUDE_CODE_TESTS").is_none() {
        eprintln!(
            "claude-code: skipped, MUSTER_CLAUDE_CODE_TESTS is not set; ./dev --claude-code sets it"
        );
        return;
    }
    let arguments = how_to_run().unwrap_or_else(|why| {
        panic!(
            "claude-code: could not run Claude Code: {why}.\n  Impact: nothing recorded or \
             checked how Claude Code treats the daemon's wake, so this tier did not pass.\n  \
             Fix: install claude and log in (`claude auth login`), or set ANTHROPIC_API_KEY."
        )
    });
    let version = command_output("claude", &["--version"])
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    let recording = std::env::var_os("MUSTER_RECORD_CLAUDE_INBOX").is_some();
    let expected = if recording {
        None
    } else {
        Some(newest_recording().unwrap_or_else(|| {
            panic!(
                "claude-code: no recording of Claude Code's inbox under {CORPUS}.\n  Impact: \
                 nothing to hold Claude Code {version} to.\n  Fix: record one with \
                 MUSTER_RECORD_CLAUDE_INBOX=1 ./dev --claude-code."
            )
        }))
    };

    let home = std::env::var("HOME").expect("HOME is set");
    let mut environment = vec![("HOME", home), ("USER", std::env::var("USER").unwrap_or_default())];
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        environment.push(("ANTHROPIC_API_KEY", key));
    }
    let environment: Vec<(&str, &str)> =
        environment.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let daemon = daemon_with(&environment);
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());

    let mut sessions = Vec::new();
    for (index, case) in CASES.iter().enumerate() {
        let project = daemon.root().join(case.name);
        std::fs::create_dir_all(&project).unwrap();
        let settings = project.join("settings.json");
        std::fs::write(&settings, case.settings().to_string()).unwrap();
        let mut command: Vec<String> = arguments.iter().map(|argument| quoted(argument)).collect();
        command.push(format!("--settings {}", quoted(&settings.display().to_string())));
        if case.bypass {
            command.push("--permission-mode bypassPermissions".to_string());
        }
        make(
            &mut control,
            proto::pane_request::Create {
                // Nothing pane-shaped reaches the session, so it is a session in a plain
                // terminal as far as anything it runs can tell.
                command: Some(format!("env -u MUSTER_PANE claude {}", command.join(" "))),
                cwd: Some(project.display().to_string()),
                grid: Some(proto::Grid { cols: 110, rows: 35, width_px: 1100, height_px: 700 }),
                ..create(case.name, in_new_tab(&format!("t{index}")))
            },
        );
        sessions.push((*case, project));
    }

    let mut seen = Vec::new();
    for (case, project) in &sessions {
        until_ready(&mut control, &mut input, case.name);
        let (socket, token) = inbox_of(project);
        seen.push(observe(&mut control, case.name, *case, &socket, &token));
    }
    for seen in &seen {
        eprintln!("claude-code: {}: {}", seen.case.describe(), seen.outcome.name());
        let bare = Outcome::shown_in(&seen.bare_screen);
        assert_eq!(bare, None, "{}: a bare connection showed something", seen.case.name);
    }

    if let Some((recorded_version, recorded)) = expected {
        let recorded: BTreeMap<String, String> = recorded["cases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|case| {
                Some((case["case"].as_str()?.to_string(), case["outcome"].as_str()?.to_string()))
            })
            .collect();
        for seen in &seen {
            let was = recorded.get(seen.case.name).map_or("not recorded", String::as_str);
            assert_eq!(
                seen.outcome.name(),
                was,
                "Claude Code {version} treats a message from outside the session differently \
                 from the recording of {recorded_version}, for {}.\n  Impact: the daemon's \
                 inbox adapter rests on that recording, so agents it wakes may now be left \
                 unwoken or held for approval.\n  Fix: re-record with \
                 MUSTER_RECORD_CLAUDE_INBOX=1 ./dev --claude-code, then revisit \
                 docs/observations/claude-code-{version}.md and MIP-4's inbox adapter default.",
                seen.case.describe(),
            );
        }
    } else {
        record(&version, &seen);
    }
}
