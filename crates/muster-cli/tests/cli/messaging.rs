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
    beside, close_request, create, expect, in_new_tab, make, session, until_text,
};
use muster_harness::{Daemon, Input};
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
    assert_eq!(daemon.replace(None).outcome(), proto::Outcome::Done);
    // Heard or not depends on whether the wait was asked again before the post: either way
    // the message is unread, and the wait returns with it.
    let posted = muster(&daemon, &["msg", "--as", "a", "post", "after", "the", "handover"]);
    assert!(matches!(posted.status.code(), Some(0 | 6)), "{}", complained(&posted));

    let waited = waiting.join().unwrap();
    assert_eq!(ok(&waited), "[muster] g: 1 new (#4), from a. Read: muster msg read --group g");
}

#[test]
fn no_daemon_to_ask_exits_3() {
    let nowhere =
        std::env::temp_dir().join(format!("muster-no-daemon-{}.sock", std::process::id()));
    let ran = muster_with(&nowhere, &["msg", "who"], None);
    assert_eq!(ran.status.code(), Some(3), "{}", complained(&ran));
    assert!(complained(&ran).contains("no muster-daemon answered"), "{}", complained(&ran));
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
    let to_all = ["msg", "--as", "lead", "post", "--to", "p1,p2,p3,p4"];
    assert_eq!(
        ok(&muster(&daemon, &[&to_all[..], &["a", "brief"]].concat())),
        "posted #7 to lead+p1+p2+p3+p4\n\
         woke: p1 (idle)\n\
         rung once idle: p2 (working)\n\
         rung once its prompt is empty: p3 (idle)\n\
         rung once an agent is found: p4"
    );
    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "lead", "who"])),
        "lead  gone             lead+p1+p2+p3+p4\n\
         p1    alive (idle)     lead+p1+p2+p3+p4\n\
         p2    alive (working)  lead+p1+p2+p3+p4\n\
         p3    alive (idle)     lead+p1+p2+p3+p4\n\
         p4    gone             lead+p1+p2+p3+p4"
    );

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

    assert_eq!(
        ok(&muster(&daemon, &[&to_all[..], &["more"]].concat())),
        "posted #8 to lead+p1+p2+p3+p4\n\
         woke: p1 (idle, already woken), p2 (working, already woken), \
         p3 (idle, already woken), p4 (already woken)"
    );
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
/// on across a handover to a new daemon, since a transcript pane outlives any one daemon.
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
    assert_eq!(daemon.replace(None).outcome(), proto::Outcome::Done);
    let posted = muster(&daemon, &["msg", "--as", "b", "post", "after", "the", "handover"]);
    assert!(matches!(posted.status.code(), Some(0 | 6)), "{}", complained(&posted));
    until_line("after the handover");

    following.kill().expect("the follow is ours to end");
    let _ = following.wait();
}

/// An agent whose pane has closed can be woken by nothing, so a post to it alone is unheard.
#[test]
fn a_post_to_an_agent_whose_pane_closed_is_unheard() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", proto::Side::Right)));
    daemon.run_agent("p1");
    ok(&muster(&daemon, &["msg", "--as", "lead", "post", "--to", "p1", "a", "brief"]));
    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    assert_eq!(
        unheard(&muster(&daemon, &["msg", "--as", "lead", "post", "--to", "p1", "more"])),
        "posted #5 to lead+p1\nnot woken: p1 (no agent in its pane)"
    );
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
    ok(&muster(&daemon, &["msg", "--as", "director", "join", "--group", "council"]));
    ok(&muster(&daemon, &["msg", "--as", "builder", "join"]));
    ok(&muster(&daemon, &["msg", "--as", "critic", "join"]));
    assert_eq!(
        ok(&muster(&daemon, &["msg", "group", "add", "council", "builder", "critic"])),
        "added builder, critic to council"
    );

    let left = muster(&daemon, &["msg", "--as", "builder", "leave", "--group", "council"]);
    assert_eq!(left.status.code(), Some(1), "{}", said(&left));
    assert!(complained(&left).contains("only director or @human may"), "{}", complained(&left));
    let aside = ["msg", "--as", "builder", "post", "--group", "council", "--to", "critic", "hi"];
    let aside = muster(&daemon, &aside);
    assert_eq!(aside.status.code(), Some(1), "{}", said(&aside));
    assert!(
        complained(&aside).contains("you may address director or @human"),
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
    let plan = ["msg", "--as", "director", "post", "--group", "council", "the", "plan"];
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
        "council  @human, builder, critic, director"
    );
    assert_eq!(
        ok(&muster(&daemon, &["msg", "--as", "builder", "read"])),
        "--- council #6 | critic joined ---\n\
         --- council #7 | @human paused it ---\n\
         --- council #8 | director ---\nthe plan\n--- end council #8 | director ---\n\
         --- council #9 | @human resumed it ---"
    );
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
    ok(&muster(&daemon, &["msg", "--as", "director", "join", "--group", "council"]));
    ok(&muster(&daemon, &["msg", "--as", "builder", "join"]));
    ok(&muster(&daemon, &["msg", "group", "add", "council", "builder"]));

    let refused = muster(&daemon, &["msg", "--as", "builder", "group", "delete", "council"]);
    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    assert!(complained(&refused).contains("delete it"), "{}", complained(&refused));

    let deleted = ["msg", "--as", "director", "group", "delete", "council"];
    assert_eq!(
        ok(&muster(&daemon, &deleted)),
        "deleted council (5 entries); let go: @human, builder, director"
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
