//! `muster msg` as a person or an agent types it: the real binary against a real daemon built from
//! this commit, reached through `$MUSTER_DAEMON_SOCKET` as a pane would reach it.
//!
//! The child's environment is cleared for the reason `driving_a_window.rs` gives, and more: this
//! suite may run inside a Claude Code session, whose inbox socket in the environment would make
//! every caller that session.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use std::time::Duration;

use muster_daemon_proto::{self as proto, messaging, session_request};
use muster_harness::requests::{
    beside, close_request, create, expect, in_new_tab, make, session, snapshot, until_text,
};
use muster_harness::{Daemon, Input, until_some};
use serde_json::Value;

fn muster(daemon: &Daemon, arguments: &[&str]) -> Output {
    muster_with(daemon.socket_path(), arguments, None)
}

fn muster_with(socket: &Path, arguments: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_muster"))
        .args(arguments)
        .env_clear()
        .env("HOME", std::env::temp_dir())
        .env("MUSTER_DAEMON_SOCKET", socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the muster binary runs");
    if let Some(text) = stdin {
        child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    }
    drop(child.stdin.take());
    child.wait_with_output().expect("the muster binary finishes")
}

fn said(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim_end().to_string()
}

fn complained(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim_end().to_string()
}

#[track_caller]
fn ok(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{}", complained(output));
    said(output)
}

/// A post that was kept and woke nobody live: here, because `--as` names have no session for
/// the daemon to wake.
#[track_caller]
fn unheard(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(6), "{}", complained(output));
    said(output)
}

#[test]
fn two_agents_post_read_and_are_held_to_the_guard() {
    let daemon = Daemon::start_built();
    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "a", "join", "--group", "review"])),
        "created review and joined it as a"
    );
    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "b", "join", "--group", "review"])),
        "joined review as b"
    );

    let posted =
        unheard(&muster(&daemon, &["msg", "--as", "a", "post", "the", "parser", "is", "in"]));
    assert_eq!(posted, "posted #4 to review\nnot woken: b (sees it when it reads)");

    let refused = muster(&daemon, &["msg", "--as", "b", "post", "done"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        complained(&refused).contains("muster msg read --group review"),
        "the refusal names the read that clears it: {}",
        complained(&refused)
    );

    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "b", "read"])),
        "--- review #4 | a ---\nthe parser is in\n--- end review #4 | a ---"
    );
    assert_eq!(ok(&muster(&daemon, &["msg", "--as", "b", "read"])), "nothing unread");
    assert_eq!(ok(&muster(&daemon, &["msg", "--as", "b", "read", "--if-unread"])), "");
    unheard(&muster(&daemon, &["msg", "--as", "b", "post", "--to", "a", "done"]));

    let log = ok(&muster(&daemon, &["msg", "--json", "log", "--group", "review"]));
    let log: Value = serde_json::from_str(&log).unwrap();
    let bodies: Vec<&str> = log["groups"][0]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["body"].as_str())
        .collect();
    assert_eq!(bodies, ["the parser is in", "done"]);
}

/// A question offers answers with `--option`, `read` numbers them, and `--answer` posts the
/// reply that picks one, to whoever asked; a second pick is refused, and a page of the log ends
/// on the reply.
#[test]
fn a_question_is_offered_read_and_answered_from_the_command_line() {
    let daemon = Daemon::start_built();
    ok(&muster(&daemon, &["msg", "--as", "a", "join", "--group", "review"]));
    ok(&muster(&daemon, &["msg", "--as", "b", "join", "--group", "review"]));
    let asked = ["msg", "--as", "a", "post", "--to", "b", "--option", "Run it now"];
    let asked = [&asked[..], &["--option", "Skip it", "run the contract tier?"]].concat();
    assert_eq!(unheard(&muster(&daemon, &asked)).lines().next(), Some("posted #4 to review"));

    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "b", "read"])),
        "--- review #4 | a -> b ---\nrun the contract tier?\n\n\
         Answers (reply with `muster msg post --group review --answer 4:N`):\n\
         1. Run it now\n2. Skip it\n--- end review #4 | a ---"
    );
    let reply =
        ["msg", "--as", "b", "post", "--group", "review", "--answer", "4:2", "after", "lunch"];
    assert_eq!(unheard(&muster(&daemon, &reply)).lines().next(), Some("posted #5 to review"));
    let again =
        muster(&daemon, &["msg", "--as", "b", "post", "--group", "review", "--answer", "4:1"]);
    assert_eq!(again.status.code(), Some(1));
    assert!(complained(&again).contains("answered already"), "{}", complained(&again));

    let page = ok(&muster(&daemon, &["msg", "--json", "log", "--group", "review", "--last", "1"]));
    let page: Value = serde_json::from_str(&page).unwrap();
    let group = &page["groups"][0];
    assert_eq!(group["earlier"], true);
    let reply = &group["entries"][0];
    assert_eq!(reply["to"], serde_json::json!(["a"]));
    assert_eq!(reply["body"], "Answer: Skip it\n\nafter lunch");
    assert_eq!(reply["answering"], serde_json::json!({"question": 4, "answer": "Skip it"}));
}

/// `--if-unread` is for a hook after every tool call, whose output the model is handed: a join
/// or a leave is nothing to interrupt it with.
#[test]
fn read_if_unread_says_nothing_when_only_joins_and_leaves_are_new() {
    let daemon = Daemon::start_built();
    ok(&muster(&daemon, &["msg", "--as", "a", "join", "--group", "g"]));
    ok(&muster(&daemon, &["msg", "--as", "b", "join", "--group", "g"]));
    ok(&muster(&daemon, &["msg", "--as", "c", "join", "--group", "g"]));
    assert_eq!(ok(&muster(&daemon, &["msg", "--as", "b", "read", "--if-unread"])), "");

    unheard(&muster(&daemon, &["msg", "--as", "c", "post", "hello"]));
    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "a", "read", "--if-unread"])),
        "--- g #3 | b joined ---\n--- g #4 | c joined ---\n--- g #5 | c ---\nhello\n--- end g #5 | c ---"
    );
}

#[test]
fn a_body_comes_whole_from_a_file_or_stdin() {
    let daemon = Daemon::start_built();
    ok(&muster(&daemon, &["msg", "--as", "a", "join", "--group", "g"]));
    ok(&muster(&daemon, &["msg", "--as", "b", "join", "--group", "g"]));
    // Longer than Claude Code folds a paste at, with the lines a brief has.
    let brief = "a line of the brief\n".repeat(400);
    let file = daemon.root().join("brief.md");
    std::fs::write(&file, &brief).unwrap();

    unheard(&muster(&daemon, &["msg", "--as", "a", "post", "--file", &file.display().to_string()]));
    unheard(&muster_with(
        daemon.socket_path(),
        &["msg", "--as", "a", "post", "-"],
        Some("from stdin"),
    ));

    let read = ok(&muster(&daemon, &["msg", "--json", "--as", "b", "read"]));
    let read: Value = serde_json::from_str(&read).unwrap();
    let bodies: Vec<&str> = read["groups"][0]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["body"].as_str())
        .collect();
    assert_eq!(bodies, [brief.as_str(), "from stdin"]);
}

#[test]
fn a_wait_prints_the_wake_and_a_timeout_exits_5() {
    let daemon = Daemon::start_built();
    ok(&muster(&daemon, &["msg", "--as", "a", "join", "--group", "g"]));
    ok(&muster(&daemon, &["msg", "--as", "b", "join", "--group", "g"]));

    let timed_out = muster(&daemon, &["msg", "--as", "b", "wait", "--timeout", "1"]);
    assert_eq!(timed_out.status.code(), Some(5), "{}", complained(&timed_out));

    unheard(&muster(&daemon, &["msg", "--as", "a", "post", "go"]));
    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "b", "wait"])),
        "[muster] g: 1 new (#4), from a. Read: muster msg read --group g"
    );
}

/// A handover ends every wait in progress; the CLI asks the new daemon again, and the wait is
/// answered by a post made to it, however long the wait had run.
#[test]
fn a_wait_in_progress_when_the_daemon_hands_over_is_answered_by_the_new_one() {
    let mut daemon = Daemon::start_built();
    ok(&muster(&daemon, &["msg", "--as", "a", "join", "--group", "g"]));
    ok(&muster(&daemon, &["msg", "--as", "b", "join", "--group", "g"]));
    let mut logging = daemon.connect();
    let follow = session_request::Request::FollowLog(session_request::FollowLog { after: None });
    expect(&mut logging, session(follow), proto::Outcome::Done);

    let socket = daemon.socket_path().to_path_buf();
    let waiting = std::thread::spawn(move || {
        muster_with(&socket, &["msg", "--as", "b", "wait", "--timeout", "60"], None)
    });
    logging.logged_until("msg.waiting", Duration::from_secs(20));
    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    // Heard or not depends on whether the wait was asked again before the post: either way
    // the message is unread, and the wait returns with it.
    let posted = muster(&daemon, &["msg", "--as", "a", "post", "after", "the", "handover"]);
    assert!(matches!(posted.status.code(), Some(0 | 6)), "{}", complained(&posted));

    let waited = waiting.join().unwrap();
    assert_eq!(ok(&waited), "[muster] g: 1 new (#4), from a. Read: muster msg read --group g");
}

/// A socket somebody named is a daemon somebody chose, so nothing is started there.
#[test]
fn no_daemon_at_a_named_socket_exits_3_and_starts_none() {
    let nowhere =
        std::env::temp_dir().join(format!("muster-no-daemon-{}.sock", std::process::id()));
    let ran = muster_with(&nowhere, &["msg", "who"], None);
    assert_eq!(ran.status.code(), Some(3), "{}", complained(&ran));
    assert!(complained(&ran).contains("no muster-daemon answered"), "{}", complained(&ran));
    assert!(complained(&ran).contains("none was started"), "{}", complained(&ran));
    assert!(!nowhere.exists());
}

/// A `muster` laid out as an install lays it out, with a scratch Muster home and no daemon
/// running: the CLI and the daemon side by side, as a build and a devenv install hold them, and
/// the daemon's data beside it, under the target directory, where a debug daemon finds
/// libghostty-vt by a path relative to itself. Whatever daemon a test started is stopped when it
/// ends.
///
/// Copied, not hard-linked, though macOS holds a binary it has never seen for about half a
/// second while it scans it. A hard link gives every test's folder a name for one file, and dyld
/// and `current_exe` can report the file by another test's name: once that test has removed its
/// folder, a daemon resolving libghostty-vt through it aborts before it answers, and a CLI looks
/// for its daemon in the wrong install.
struct Installed {
    folder: std::path::PathBuf,
    home: std::path::PathBuf,
}

impl Installed {
    fn new(with_a_daemon: bool) -> Installed {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let folder = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("start-{}-{n}", std::process::id()));
        // Short, since the daemon's socket goes under it.
        let home =
            std::path::PathBuf::from(format!("/tmp/muster-test/s{}-{n}", std::process::id()));
        for directory in [&folder, &home] {
            let _ = std::fs::remove_dir_all(directory);
            std::fs::create_dir_all(directory).unwrap();
        }
        place(Path::new(env!("CARGO_BIN_EXE_muster")), &folder.join("muster"));
        if with_a_daemon {
            place(&muster_harness::built_daemon(), &folder.join("muster-daemon"));
            std::os::unix::fs::symlink(
                muster_harness::DAEMON_DATA,
                folder.join("muster-daemon-data"),
            )
            .unwrap();
        }
        Installed { folder, home }
    }

    fn socket(&self) -> std::path::PathBuf {
        proto::install::socket_path(&self.home)
    }

    fn muster(&self, arguments: &[&str]) -> Output {
        Command::new(self.folder.join("muster"))
            .args(arguments)
            .env_clear()
            .env("HOME", &self.home)
            .env("MUSTER_HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .output()
            .expect("the muster binary runs")
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        let _ = muster_daemon_client::launch::stop(&self.socket(), Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&self.folder);
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn place(from: &Path, to: &Path) {
    std::fs::copy(from, to).unwrap();
}

/// MIP-4 section 1: with no daemon running, a msg verb starts this install's, on the socket a
/// window would look for it on and written down where a window writes its own, then asks it.
#[test]
fn a_msg_verb_with_no_daemon_running_starts_one() {
    let installed = Installed::new(true);
    let first = installed.muster(&["msg", "who"]);
    assert_eq!(first.status.code(), Some(0), "{}", complained(&first));
    assert!(
        complained(&first).contains("no muster-daemon was running")
            && complained(&first).contains("so this is starting one"),
        "{}",
        complained(&first)
    );
    let records = installed.home.join("state").join("daemons");
    let written: Vec<String> = std::fs::read_dir(&records)
        .unwrap_or_else(|error| panic!("no record under {}: {error}", records.display()))
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    let socket = installed.socket().display().to_string();
    assert!(written.iter().any(|record| record.contains(&socket)), "{written:?}");

    let again = installed.muster(&["msg", "who"]);
    assert_eq!(again.status.code(), Some(0), "{}", complained(&again));
    assert_eq!(complained(&again), "", "a running daemon is asked, not started again");
}

/// A `muster` with no daemon beside it, as one copied onto a machine on its own would be,
/// starts nothing and says where it looked.
#[test]
fn a_muster_with_no_daemon_beside_it_says_where_it_looked() {
    let installed = Installed::new(false);
    let ran = installed.muster(&["msg", "who"]);
    assert_eq!(ran.status.code(), Some(3), "{}", complained(&ran));
    let beside = installed.folder.canonicalize().unwrap().join("muster-daemon");
    assert!(complained(&ran).contains("none was started"), "{}", complained(&ran));
    assert!(complained(&ran).contains(&beside.display().to_string()), "{}", complained(&ran));
    assert!(!installed.socket().exists());
}

#[test]
fn a_post_needs_a_message() {
    let daemon = Daemon::start_built();
    let ran = muster(&daemon, &["msg", "post"]);
    assert_eq!(ran.status.code(), Some(1));
    assert!(complained(&ran).contains("needs a message"), "{}", complained(&ran));
}

/// The verbs are spelled in one place, and the reference has to spell them the same way.
#[test]
fn the_reference_spells_every_verb_as_the_command_does() {
    let reference = include_str!("../../../../docs/cli/msg.md");
    let help = muster_with(Path::new("/nonexistent"), &["msg", "--help"], None);
    let help = said(&help);
    for verb in messaging::VERBS {
        let row = reference.lines().any(|line| {
            line.strip_prefix(&format!("| `{verb}"))
                .is_some_and(|rest| rest.starts_with(' ') || rest.starts_with('`'))
        });
        assert!(row, "docs/cli/msg.md's table of verbs has no row for `{verb}`");
        let listed = help.lines().any(|line| line.split_whitespace().next() == Some(verb));
        assert!(listed, "`muster msg --help` does not list {verb} as a command: {help}");
    }
    assert!(
        reference.contains(&messaging::command(messaging::READ, "")),
        "docs/cli/msg.md never spells the command as `muster msg read`"
    );
}

/// The daemon's quiet period: how long nothing may have been typed into a pane before it is
/// rung.
const QUIET: Duration = Duration::from_secs(3);

/// A daemon with the fake agent in panes `p1` to `p3`, idle, working and holding a half-typed
/// draft, and `p4` at its shell, where an agent may yet start. Every pane has been quiet for the
/// doorbell's quiet period.
fn agents_in_panes() -> Daemon {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    for pane in ["p2", "p3", "p4"] {
        make(&mut control, create(pane, beside("p1", proto::Side::Right)));
    }
    for pane in ["p1", "p2", "p3"] {
        daemon.run_agent(pane);
    }
    daemon.set_agent_state("p2", proto::AgentState::Working);
    Input::connect(daemon.socket_path()).send(
        "p3",
        proto::input_event::Input::Send(proto::input_event::Send {
            text: "half a thought".to_string(),
            enter: false,
            ..Default::default()
        }),
    );
    until_text(&mut control, "p3", "half a thought");
    std::thread::sleep(QUIET + Duration::from_millis(500));
    daemon
}

/// A post to agents in panes says, for each, whether it was rung, and what a ring still to
/// come waits for; each counts as heard, and so does one already woken.
#[test]
fn a_post_to_agents_in_panes_says_when_each_is_rung() {
    let daemon = agents_in_panes();
    // Each pane's agent is shown by the word its pane was drawn, beside the pane's id.
    let words = words(&daemon);
    let (p1, p2, p3, p4) = (&words["p1"], &words["p2"], &words["p3"], &words["p4"]);
    let mut pair = ["lead", p1, p2, p3, p4];
    pair.sort_unstable();
    let pair = pair.join("+");
    let to_all = ["msg", "--as", "lead", "post", "--to", "p1,p2,p3,p4"];
    assert_eq!(
        ok(&muster(&daemon, &[&to_all[..], &["a", "brief"]].concat())),
        format!(
            "posted #7 to {pair}\n\
             woke: {p1} (p1, idle)\n\
             rung once idle: {p2} (p2, working)\n\
             rung once its prompt is empty: {p3} (p3, idle, its prompt holds something typed \
             and not sent)\n\
             rung once an agent is found: {p4} (p4)"
        )
    );
    let shown: Vec<String> =
        ["p1", "p2", "p3", "p4"].iter().map(|pane| format!("{} ({pane})", words[*pane])).collect();
    let width = shown.iter().map(String::len).max().unwrap_or_default();
    let mut who = vec![format!("{:<width$}  {:<15}  {pair}", "lead", "gone")];
    for (name, state) in
        shown.iter().zip(["alive (idle)", "alive (working)", "alive (idle)", "gone"])
    {
        who.push(format!("{name:<width$}  {state:<15}  {pair}"));
    }
    let said = ok(&muster(&daemon, &["msg", "--as", "lead", "who"]));
    let members: Vec<&str> = said.lines().filter(|line| !line.starts_with(' ')).collect();
    assert_eq!(members, who, "{said}");
    // Under each member, why #7 waits on it. The idle agent's ring may or may not have been
    // taken by now, so only the others' are fixed.
    for (name, why) in [
        (
            &shown[1],
            "it is at work, so it is rung once it is idle; an urgent post reaches it at work",
        ),
        (&shown[2], "a wake for it is held because its prompt holds something typed and not sent"),
        (&shown[3], "no agent is in its pane any more"),
    ] {
        let row = format!("{name}  ");
        let under = said
            .split('\n')
            .skip_while(|line| !line.starts_with(&row))
            .nth(1)
            .unwrap_or_else(|| panic!("nothing under {name}: {said}"));
        assert_eq!(under, format!("  has not read #7 in {pair}, from lead: {why}"), "{said}");
    }

    let json = ["msg", "--json", "--as", "other", "post", "--to", "p2,p3,p4", "another"];
    let posted: Value = serde_json::from_str(&ok(&muster(&daemon, &json))).unwrap();
    assert_eq!(posted["deferred"], serde_json::json!(["p2", "p3", "p4"]), "{posted}");
    assert_eq!(
        posted["until"],
        serde_json::json!({ "p2": "idle", "p3": "prompt", "p4": "agent" }),
        "{posted}"
    );
    assert_eq!(posted["doing"], serde_json::json!({ "p2": "working", "p3": "idle" }), "{posted}");
    for none in ["woke", "already_woken", "gone", "no_agent", "no_doorbell", "waiting"] {
        assert_eq!(posted[none], serde_json::json!([]), "{none}: {posted}");
    }

    // Woken already, an idle agent is rung again, since nothing on its way will make it read;
    // one at work, or with no agent in its pane yet, is left to the wake it has.
    assert_eq!(
        ok(&muster(&daemon, &[&to_all[..], &["more"]].concat())),
        format!(
            "posted #8 to {pair}\n\
             woke: {p1} (p1, idle), {p2} (p2, working, already woken), {p4} (p4, already woken)\n\
             rung once its prompt is empty: {p3} (p3, idle, its prompt holds something typed \
             and not sent)"
        )
    );
}

/// What each pane is called: the word its daemon drew, since nothing here names one.
fn words(daemon: &Daemon) -> std::collections::BTreeMap<String, String> {
    snapshot(&mut daemon.connect())
        .panes
        .into_iter()
        .filter_map(|pane| Some((pane.pane.clone(), pane.callsign?)))
        .collect()
}

/// The human is heard: whoever reads the post there is the person at the keyboard.
#[test]
fn a_post_to_the_human_is_heard() {
    let daemon = Daemon::start_built();
    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "a", "post", "--to", "@human", "look"])),
        "posted #4 to @human+a\nnot woken: @human (notified when a window opens)"
    );
}

/// The transcript: `log --follow` prints what is there, then each entry as it lands, and goes
/// on across a handover to a new daemon, since a terminal or script following a group outlives
/// any one daemon.
#[test]
fn a_followed_log_prints_each_entry_as_it_lands_across_a_handover() {
    use std::io::{BufRead, BufReader};
    let mut daemon = Daemon::start_built();
    ok(&muster(&daemon, &["msg", "--as", "a", "join", "--group", "g"]));
    // Alone in the group, a wakes nobody with it, which exits 6.
    unheard(&muster(&daemon, &["msg", "--as", "a", "post", "before"]));

    let mut following = Command::new(env!("CARGO_BIN_EXE_muster"))
        .args(["msg", "log", "--group", "g", "--follow"])
        .env_clear()
        .env("HOME", std::env::temp_dir())
        .env("MUSTER_DAEMON_SOCKET", daemon.socket_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the muster binary runs");
    let (lines, heard) = std::sync::mpsc::channel();
    let stdout = following.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                return;
            }
        }
    });
    let until_line = |wanted: &str| loop {
        match heard.recv_timeout(Duration::from_secs(20)) {
            Ok(line) if line == wanted => return,
            Ok(_) => {}
            Err(error) => panic!("the followed log never printed {wanted:?}: {error}"),
        }
    };
    until_line("before");

    ok(&muster(&daemon, &["msg", "--as", "b", "join", "--group", "g"]));
    until_line("--- g #4 | b joined ---");
    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    let posted = muster(&daemon, &["msg", "--as", "b", "post", "after", "the", "handover"]);
    assert!(matches!(posted.status.code(), Some(0 | 6)), "{}", complained(&posted));
    until_line("after the handover");

    following.kill().expect("the follow is ours to end");
    let _ = following.wait();
}

/// An agent whose pane has closed can be woken by nothing, so a post to it alone is unheard. It is
/// shown by the name its pane had, which is free for another pane by now, beside the pane's id,
/// which still reaches it.
#[test]
fn a_post_to_an_agent_whose_pane_closed_is_unheard() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", proto::Side::Right)));
    daemon.run_agent("p1");
    let word = words(&daemon)["p1"].clone();
    let pair = if word.as_str() < "lead" { format!("{word}+lead") } else { format!("lead+{word}") };
    ok(&muster(&daemon, &["msg", "--as", "lead", "post", "--to", &word, "a", "brief"]));
    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    assert_eq!(
        unheard(&muster(&daemon, &["msg", "--as", "lead", "post", "--to", "p1", "more"])),
        format!("posted #5 to {pair}\nnot woken: {word} (p1, no agent in its pane)")
    );
}

/// A `muster msg` run under an agent's server is refused, saying which server and what to do: the
/// server runs every session's commands with the environment of the one that started it, as
/// Codex's shared app-server does, so the pane it names is no more its pane than any other. A
/// post taken would be p1's, and a read would hand p1's messages to whichever session asked.
#[test]
fn a_msg_request_through_an_agents_server_is_refused_and_says_what_to_do() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", proto::Side::Right)));
    daemon.run_agent("p1");
    daemon.run_agent("p2");
    let home = daemon.root().join("home");
    let served = |line: &str, count: usize| {
        let cli = env!("CARGO_BIN_EXE_muster");
        daemon.type_into("p1", &format!("served-run {cli} {line}"));
        let reported = until_some(&format!("{count} commands through the server"), || {
            let reported = std::fs::read_to_string(home.join("fake-agent-reported")).ok()?;
            let lines: Vec<String> = reported.lines().map(str::to_string).collect();
            (lines.len() >= count).then_some(lines)
        });
        let said = std::fs::read_to_string(home.join("fake-agent-served-said")).unwrap_or_default();
        (reported[count - 1].clone(), said)
    };

    for (count, line) in [(1, "msg post --to p2 hello"), (2, "msg read")] {
        let (status, said) = served(line, count);
        assert_eq!(status, "served 1", "`{line}` through the server was taken: {said}");
        assert!(said.contains("this request came through `claude serve`"), "{said}");
        assert!(said.contains("run the fake agent without its server"), "{said}");
    }

    // p1's own post, not through the server, is the pair group's first: nothing was posted for it.
    let word = words(&daemon)["p2"].clone();
    let as_p1 = Command::new(env!("CARGO_BIN_EXE_muster"))
        .args(["msg", "post", "--to", &word, "hello"])
        .env_clear()
        .env("HOME", std::env::temp_dir())
        .env("MUSTER_DAEMON_SOCKET", daemon.socket_path())
        .env("MUSTER_PANE", "p1")
        .stdin(Stdio::null())
        .output()
        .expect("the muster binary runs");
    let posted = ok(&as_p1);
    assert!(posted.starts_with("posted #4 to "), "{posted}");
}

/// An agent nested in a pane, as a `claude -p` that the pane's agent started is, inherits the
/// pane's environment. It posts as the pane, since it speaks for the agent that started it, but
/// its messaging hooks' reads and waits are refused: taken, they would hand the pane's messages to
/// the nested session and mark them read, and the pane's own agent would never see them.
#[test]
fn a_nested_agent_posts_as_its_pane_but_does_not_read_or_wait_for_it() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", proto::Side::Right)));
    daemon.run_agent("p1");
    daemon.run_agent("p2");
    let home = daemon.root().join("home");
    let nested = |line: &str, count: usize| {
        let cli = env!("CARGO_BIN_EXE_muster");
        daemon.type_into("p1", &format!("nested-run {cli} {line}"));
        let reported = until_some(&format!("{count} commands from the nested agent"), || {
            let reported = std::fs::read_to_string(home.join("fake-agent-reported")).ok()?;
            let lines: Vec<String> = reported.lines().map(str::to_string).collect();
            (lines.len() >= count).then_some(lines)
        });
        let said = std::fs::read_to_string(home.join("fake-agent-nested-said")).unwrap_or_default();
        (reported[count - 1].clone(), said)
    };
    let in_pane = |pane: &str, arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_muster"))
            .args(arguments)
            .env_clear()
            .env("HOME", std::env::temp_dir())
            .env("MUSTER_DAEMON_SOCKET", daemon.socket_path())
            .env("MUSTER_PANE", pane)
            .stdin(Stdio::null())
            .output()
            .expect("the muster binary runs")
    };
    let p2 = words(&daemon)["p2"].clone();
    let p1 = words(&daemon)["p1"].clone();

    let (status, said) = nested(&format!("msg post --to {p2} from-the-nested-agent"), 1);
    assert_eq!(status, "nested 0", "the nested agent's post was refused: {said}");
    assert!(ok(&in_pane("p2", &["msg", "read"])).contains("from-the-nested-agent"));
    ok(&in_pane("p2", &["msg", "post", "--to", &p1, "for-the-pane"]));

    for (count, line) in [(2, "msg read --if-unread"), (3, "msg wait --due --timeout 1")] {
        let (status, said) = nested(line, count);
        assert_eq!(status, "nested 1", "`{line}` from the nested agent was taken: {said}");
        assert!(said.contains("outside the process group the pane's own agent runs in"), "{said}");
        assert!(said.contains("env -u MUSTER_PANE"), "{said}");
    }
    let read = ok(&in_pane("p1", &["msg", "read"]));
    assert!(read.contains("for-the-pane"), "the pane's agent lost its message: {read}");
}

/// A nested `claude -p` has an inbox of its own, which a request taken as the pane would give the
/// pane's participant in place of its own: a wake sent there ends with the nested session. So its
/// posts carry the pane alone, and one naming another participant, as `join --name` does, is a
/// session of its own and leaves the pane where it was.
#[test]
fn a_nested_agent_gives_its_pane_none_of_its_addresses() {
    use proto::msg_answer::Answer;
    use proto::msg_request::{Caller, Request, Who};
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", proto::Side::Right)));
    daemon.run_agent("p1");
    daemon.run_agent("p2");
    let home = daemon.root().join("home");
    let inbox = daemon.root().join("nested.inbox.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&inbox).unwrap();
    let nested = |line: &str, count: usize| {
        let cli = env!("CARGO_BIN_EXE_muster");
        let with_inbox = format!("/usr/bin/env CLAUDE_CODE_MESSAGING_SOCKET={}", inbox.display());
        daemon.type_into("p1", &format!("nested-run {with_inbox} {cli} {line}"));
        let reported = until_some(&format!("{count} commands from the nested agent"), || {
            let reported = std::fs::read_to_string(home.join("fake-agent-reported")).ok()?;
            let lines: Vec<String> = reported.lines().map(str::to_string).collect();
            (lines.len() >= count).then_some(lines)
        });
        let said = std::fs::read_to_string(home.join("fake-agent-nested-said")).unwrap_or_default();
        assert_eq!(reported[count - 1], "nested 0", "`{line}` was refused: {said}");
    };
    let addresses = |control: &mut muster_harness::Control| {
        let who = proto::request::Service::Msg(proto::MsgRequest {
            caller: Some(Caller { as_name: Some("reader".into()), ..Caller::default() }),
            request: Some(Request::Who(Who { group: None })),
        });
        let asked = expect(control, who, proto::Outcome::Done);
        let Some(proto::answer::Detail::Msg(answer)) = &asked.answer.detail else { panic!() };
        let Some(Answer::Members(members)) = &answer.answer else { panic!() };
        members
            .members
            .iter()
            .map(|member| (member.name.clone(), (member.pane.clone(), member.inbox.clone())))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let p2 = words(&daemon)["p2"].clone();
    let socket = Some(inbox.display().to_string());

    nested(&format!("msg post --to {p2} from-the-nested-agent"), 1);
    assert_eq!(addresses(&mut control)["p1"], (Some("p1".to_string()), None), "after a post");

    nested("msg join --name helper --group g", 2);
    let after = addresses(&mut control);
    assert_eq!(after["p1"], (Some("p1".to_string()), None), "the pane moved to the nested name");
    assert_eq!(after["helper"], (None, socket), "the nested session is not its own");
}

/// A msg request that comes before the daemon's first manifest load cannot be attributed, so it
/// is taken even through a server, and the daemon says why once, as it does for a report. The load
/// here reads an override that never ends, as one on a mount that hangs does: a FIFO nobody opens.
#[test]
fn a_msg_request_before_the_manifests_load_is_taken_and_says_why() {
    let home = std::env::temp_dir().join(format!("muster-msg-unattributed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join("agent-detection")).unwrap();
    let fifo = home.join("agent-detection/claude.toml");
    let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: mkfifo reads a NUL-terminated path the test owns.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let daemon = Daemon::start_detecting_with(&[("MUSTER_HOME", home.to_str().unwrap())]);
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");

    let (agent, cli) = (daemon.agent_path(), env!("CARGO_BIN_EXE_muster"));
    daemon.type_into("p1", &format!("{} serve run {cli} msg read &", agent.display()));
    let reported = daemon.root().join("home/fake-agent-reported");
    let said = until_some("the read through the server to answer", || {
        std::fs::read_to_string(&reported).ok().filter(|said| !said.is_empty())
    });
    let log = std::fs::read_to_string(daemon.root().join("daemon.log")).unwrap_or_default();
    // Lets the load finish, whatever the verdict.
    drop(std::fs::OpenOptions::new().write(true).open(&fifo));
    let _ = std::fs::remove_dir_all(&home);
    assert_eq!(said.trim(), "served 0", "a request nothing could attribute was refused");
    let logged = |event: &str| log.contains(&format!("\"event\":\"{event}\""));
    assert!(logged("msg.request.unattributed"), "the daemon did not say why: {log}");
    assert!(!logged("daemon.detect.loaded"), "the manifests loaded first: {log}");
}

/// The preset a directed council is convened with (MIP-4, section 8).
const DIRECTED: &str = include_str!("../../../../extras/skill/council/directed.toml");

/// A council convened with a policy holds its members to it: a member may not leave on its own
/// or address another member, and a paused council's posts wake nobody until it is resumed.
#[test]
fn a_directed_council_holds_its_members_to_its_policy() {
    let daemon = Daemon::start_built();
    let policy = daemon.root().join("directed.toml");
    std::fs::write(&policy, DIRECTED).unwrap();
    let policy = policy.display().to_string();
    let council = ["msg", "group", "new", "council", "--policy", policy.as_str()];
    assert_eq!(ok(&muster(&daemon, &council)), "made council and joined it");
    ok(&muster(&daemon, &["msg", "--as", "director-council", "join", "--group", "council"]));
    ok(&muster(&daemon, &["msg", "--as", "builder", "join"]));
    ok(&muster(&daemon, &["msg", "--as", "critic", "join"]));
    assert_eq!(
        ok(&muster(&daemon, &["msg", "group", "add", "council", "builder", "critic"])),
        "added builder, critic to council"
    );

    let left = muster(&daemon, &["msg", "--as", "builder", "leave", "--group", "council"]);
    assert_eq!(left.status.code(), Some(1), "{}", said(&left));
    assert!(
        complained(&left).contains("only director-council or @human may"),
        "{}",
        complained(&left)
    );
    let aside = ["msg", "--as", "builder", "post", "--group", "council", "--to", "critic", "hi"];
    let aside = muster(&daemon, &aside);
    assert_eq!(aside.status.code(), Some(1), "{}", said(&aside));
    assert!(
        complained(&aside).contains("you may address director-council or @human"),
        "{}",
        complained(&aside)
    );

    let pausing = daemon.root().join("paused.toml");
    std::fs::write(&pausing, "paused = true\n").unwrap();
    let pausing = pausing.display().to_string();
    let set = muster(&daemon, &["msg", "group", "set", "council", "--policy", pausing.as_str()]);
    assert_eq!(set.status.code(), Some(1), "{}", said(&set));
    assert!(complained(&set).contains("muster msg pause council"), "{}", complained(&set));

    assert_eq!(ok(&muster(&daemon, &["msg", "pause", "council"])), "paused council");
    assert_eq!(ok(&muster(&daemon, &["msg", "pause", "council"])), "council was already paused");
    let plan = ["msg", "--as", "director-council", "post", "--group", "council", "the", "plan"];
    assert_eq!(
        ok(&muster(&daemon, &plan)),
        "posted #8 to council\nheld while council is paused: builder, critic"
    );
    assert_eq!(
        ok(&muster(&daemon, &["msg", "resume", "council"])),
        "resumed council\n\
         not woken: builder (sees it when it reads), critic (sees it when it reads)"
    );
    assert_eq!(
        ok(&muster(&daemon, &["msg", "groups"])),
        "council  @human, builder, critic, director-council"
    );
    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "builder", "read"])),
        "--- council #6 | critic joined ---\n\
         --- council #7 | @human paused it ---\n\
         --- council #8 | director-council ---\nthe plan\n\
         --- end council #8 | director-council ---\n\
         --- council #9 | @human resumed it ---"
    );
}

/// Two councils convened at once from the one preset each get a director of their own, named
/// for the group, where both once wanted `director`, which only one live session can hold. A
/// group whose name cannot be part of a participant's is refused, naming what it would be.
#[test]
fn two_councils_at_once_each_have_their_own_director() {
    let daemon = Daemon::start_built();
    let policy = daemon.root().join("directed.toml");
    std::fs::write(&policy, DIRECTED).unwrap();
    let policy = policy.display().to_string();
    for (group, member) in [("alpha", "builder-alpha"), ("beta", "builder-beta")] {
        let director = format!("director-{group}");
        ok(&muster(&daemon, &["msg", "--as", &director, "join"]));
        let council = ["msg", "--as", &director, "group", "new", group, "--policy", &policy];
        assert_eq!(ok(&muster(&daemon, &council)), format!("made {group} and joined it"));
        ok(&muster(&daemon, &["msg", "--as", member, "join"]));
        let add = ["msg", "--as", &director, "group", "add", group, member];
        assert_eq!(ok(&muster(&daemon, &add)), format!("added {member} to {group}"));
    }
    assert_eq!(
        ok(&muster(&daemon, &["msg", "groups"])),
        "alpha  builder-alpha, director-alpha\nbeta   builder-beta, director-beta"
    );

    let set = ["msg", "--as", "director-beta", "group", "set", "beta", "--policy", &policy];
    ok(&muster(&daemon, &set));
    let groups: Value =
        serde_json::from_str(&ok(&muster(&daemon, &["msg", "groups", "--json"]))).unwrap();
    let beta = groups.to_string();
    assert!(beta.contains(r#""membership":["director-beta","@human"]"#), "{beta}");
    assert!(!beta.contains("{group}"), "{beta}");

    let joined = ["msg", "group", "new", "a+b", "--policy", &policy];
    let refused = muster(&daemon, &joined);
    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    assert!(
        complained(&refused).contains(r#""director-a+b" cannot be a name"#),
        "{}",
        said(&refused)
    );
}

/// An agent that does not hold a name the policy's membership gives is not let make the group,
/// since as its only member it could add nobody and never leave, and it is told what would work.
#[test]
fn a_council_is_not_made_by_an_agent_its_policy_gives_no_say() {
    let daemon = Daemon::start_built();
    let policy = daemon.root().join("directed.toml");
    std::fs::write(&policy, DIRECTED).unwrap();
    let policy = policy.display().to_string();
    ok(&muster(&daemon, &["msg", "--as", "planner", "join"]));

    let council = ["msg", "--as", "planner", "group", "new", "council", "--policy", &policy];
    let refused = muster(&daemon, &council);
    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    let complaint = complained(&refused);
    assert!(complaint.contains("lets only director-council or @human add members"), "{complaint}");
    assert!(complaint.contains("muster msg join --name <name>"), "{complaint}");
    assert_eq!(ok(&muster(&daemon, &["msg", "groups"])), "no groups");

    ok(&muster(&daemon, &["msg", "--as", "director-council", "join"]));
    let council =
        ["msg", "--as", "director-council", "group", "new", "council", "--policy", &policy];
    assert_eq!(ok(&muster(&daemon, &council)), "made council and joined it");
}

/// A council's member cannot delete it, and the one its policy names can: the group, its log
/// and every member's place in it go, and the name is free again.
#[test]
fn a_group_is_deleted_by_whoever_its_policy_lets_change_it() {
    let daemon = Daemon::start_built();
    let policy = daemon.root().join("directed.toml");
    std::fs::write(&policy, DIRECTED).unwrap();
    let policy = policy.display().to_string();
    ok(&muster(&daemon, &["msg", "group", "new", "council", "--policy", policy.as_str()]));
    ok(&muster(&daemon, &["msg", "--as", "director-council", "join", "--group", "council"]));
    ok(&muster(&daemon, &["msg", "--as", "builder", "join"]));
    ok(&muster(&daemon, &["msg", "group", "add", "council", "builder"]));

    let refused = muster(&daemon, &["msg", "--as", "builder", "group", "delete", "council"]);
    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    assert!(complained(&refused).contains("delete it"), "{}", complained(&refused));

    let deleted = ["msg", "--as", "director-council", "group", "delete", "council"];
    assert_eq!(
        ok(&muster(&daemon, &deleted)),
        "deleted council (5 entries); let go: @human, builder, director-council"
    );
    let groups = ok(&muster(&daemon, &["msg", "groups"]));
    assert!(!groups.contains("council"), "{groups}");
    let gone = muster(&daemon, &["msg", "log", "--group", "council"]);
    assert_eq!(gone.status.code(), Some(1), "{}", said(&gone));
    let json = ok(&muster(&daemon, &["msg", "group", "new", "council", "--json"]));
    assert!(json.contains("council"), "the name is free again: {json}");
}

/// A policy file with a key the service does not know is refused before anything is sent,
/// rather than read as a policy that allows more than its author meant.
#[test]
fn a_policy_file_with_a_key_nobody_reads_is_refused() {
    let daemon = Daemon::start_built();
    let policy = daemon.root().join("typo.toml");
    std::fs::write(&policy, "membrship = [\"director\"]\n").unwrap();
    let made =
        muster(&daemon, &["msg", "group", "new", "g", "--policy", &policy.display().to_string()]);
    assert_eq!(made.status.code(), Some(1), "{}", said(&made));
    assert!(complained(&made).contains("is not a policy"), "{}", complained(&made));
    assert_eq!(ok(&muster(&daemon, &["msg", "groups"])), "no groups");
}

/// A daemon that accepts and never answers - stopped, or deadlocked - fails a `msg` verb within
/// the handshake's bound, and is not taken for an empty socket that wants a daemon started.
#[test]
fn a_daemon_that_accepts_and_never_answers_fails_the_verb_within_seconds() {
    let root = std::env::temp_dir().join(format!("muster-msg-silent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let socket = root.join("daemon.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        // Held, not read: each connection stays open and nothing ever comes back.
        let mut held = Vec::new();
        for stream in listener.incoming() {
            held.push(stream);
        }
    });

    let started = std::time::Instant::now();
    let read = muster_with(&socket, &["msg", "--as", "a", "read"], None);
    assert!(started.elapsed() < Duration::from_secs(8), "waited {:?}", started.elapsed());
    assert_eq!(read.status.code(), Some(3), "{}", complained(&read));
    assert!(
        complained(&read).contains("accepted the connection and did not answer"),
        "{}",
        complained(&read)
    );
    let _ = std::fs::remove_dir_all(&root);
}
