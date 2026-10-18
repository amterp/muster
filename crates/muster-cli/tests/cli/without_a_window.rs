//! The pane verbs with no window answering: `muster window`, `pane read`, `pane send` and
//! `pane wait`, answered by the daemon holding the panes, as they are on a devenv nothing
//! forwards a window to, or in a pane whose window has quit. The real binary against a real
//! daemon built from this commit, with `$MUSTER_SOCKET` naming a window that is not there.
//!
//! The child's environment is cleared for the reason `driving_a_window.rs` gives: this suite runs
//! inside Muster, and an inherited `MUSTER_SOCKET` is the developer's own window.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};

use muster::proto::{OpenWindow, Request, Response, Startup, request, response};
use muster_daemon_proto::{self as proto, session_request};
use muster_harness::requests::{
    beside, close_request, create, expect, in_new_tab, make, pane, session, snapshot, until_text,
};
use muster_harness::{Daemon, PATIENCE, until_some};
use prost::Message;
use serde_json::{Value, json};

/// Where a caller finds itself: a daemon, and a window it was told about that has gone.
struct Here {
    daemon: Daemon,
    /// `$MUSTER_SOCKET`: nothing listens on it.
    window: String,
    /// `$MUSTER_HOME`, with no window's socket in its state directory.
    home: PathBuf,
}

impl Here {
    fn new(daemon: Daemon) -> Here {
        let home = daemon.root().join("no-window-here");
        std::fs::create_dir_all(home.join("state")).unwrap();
        let window = daemon.root().join("quit.sock").to_string_lossy().into_owned();
        Here { daemon, window, home }
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_muster"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", &self.home)
            .env("MUSTER_HOME", &self.home)
            .env("MUSTER_SOCKET", &self.window)
            .env("MUSTER_DAEMON_SOCKET", self.daemon.socket_path())
            .stdin(Stdio::null());
        command
    }

    fn muster(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().expect("the muster binary runs")
    }

    fn spawned(&self, arguments: &[&str]) -> Child {
        self.command(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the muster binary runs")
    }
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

#[track_caller]
fn refused_with(output: &Output, code: i32) -> String {
    assert_eq!(output.status.code(), Some(code), "{}\n{}", said(output), complained(output));
    complained(output)
}

/// A daemon holding a shell in `p1` in tab `t1`, and the fake agent in `p2` and `p3` beside it.
fn panes() -> Here {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    for pane in ["p2", "p3"] {
        make(&mut control, create(pane, beside("p1", proto::Side::Right)));
    }
    for pane in ["p2", "p3"] {
        daemon.run_agent(pane);
    }
    until_text(&mut control, "p1", "$");
    Here::new(daemon)
}

#[test]
fn with_no_window_the_daemon_lists_reads_types_into_and_waits_on_its_panes() {
    let here = panes();

    let listed = ok(&here.muster(&["window"]));
    assert!(
        listed.starts_with("no window answered; the muster-daemon at")
            && listed.contains(&here.daemon.socket_path().display().to_string()),
        "the listing has to say which daemon answered, so nobody reads it as a window's:\n{listed}"
    );
    assert!(!listed.contains("(off screen)"), "with no window, nothing is off screen:\n{listed}");
    let text = listed;

    let listed: Value = serde_json::from_str(&ok(&here.muster(&["--json", "window"]))).unwrap();
    // Rows lead with each pane's callsign, which is what a reader types back.
    assert!(text.contains("t1"), "{text}");
    for pane in listed["panes"].as_array().expect("a list of panes") {
        let callsign = pane["callsign"].as_str().expect("every pane is drawn a callsign");
        assert!(text.contains(callsign), "{callsign} is not listed:\n{text}");
    }
    assert_eq!(listed["answered_by"], json!("daemon"), "{listed}");
    let p2 = listed["panes"].as_array().unwrap().iter().find(|pane| pane["pane"] == json!("p2"));
    let p2 = p2.unwrap_or_else(|| panic!("p2 is listed: {listed}"));
    assert_eq!((&p2["tab"], &p2["state"]), (&json!("t1"), &json!("idle")), "{listed}");
    the_layout_is_the_daemons_trees(&here);

    ok(&here.muster(&["pane", "send", "--pane", "p1", "--enter", "--confirm", "echo typed-in"]));
    let mut control = here.daemon.connect();
    until_text(&mut control, "p1", "typed-in\n");
    let read = ok(&here.muster(&["pane", "read", "--pane", "p1"]));
    assert!(read.contains("echo typed-in"), "the read shows what the pane printed:\n{read}");
    let last: Value = serde_json::from_str(&ok(
        &here.muster(&["--json", "pane", "read", "--pane", "p1", "--rows", "1"])
    ))
    .unwrap();
    assert_eq!((&last["rows"], &last["truncated"]), (&json!(1), &json!(true)), "{last}");

    panes_are_reached_by_their_words(&here);
    a_wait_ends_when_the_agent_gets_there(&here);
    a_wait_on_context_ends_when_the_agent_says_it(&here);
    a_compaction_is_asked_of_the_daemon(&here);
    a_send_into_a_prompt_holding_a_draft_is_refused(&here);
    a_watch_prints_each_change(&here);
    a_wait_on_a_pane_that_closes_is_refused(&here);
    a_send_the_pane_never_shows_is_refused(&here);
    keys_are_pressed_without_a_window(&here);
    a_line_the_agent_took_and_repainted_is_unanswered(&here);
    a_wait_on_an_agent_that_stops_reading_is_refused(&here);
    hiding_and_quiet_are_asked_of_the_daemon(&here);
    an_agent_is_restarted_by_the_daemon(&here);
    panes_are_made_renamed_and_closed_by_the_daemon(&here);
    helpers_are_made_by_the_daemon(&here);

    let refused = refused_with(&here.muster(&["pane", "read"]), 1);
    assert!(refused.contains("--pane"), "with no window there is no keyboard's pane:\n{refused}");
    let refused = refused_with(&here.muster(&["pane", "read", "--pane", "p9nobody"]), 1);
    assert!(refused.contains("no pane called p9nobody"), "{refused}");
    // A shell, whose agent never went to work: the daemon was asked for the turn, and said so.
    let refused = refused_with(&here.muster(&["pane", "read", "--pane", "p1", "--turn"]), 1);
    assert!(refused.contains("no turn has started"), "{refused}");
}

/// Every pane goes by a word its daemon drew until somebody names it, and with no window the CLI
/// reads that word against the daemon's own panes wherever it takes a pane's id.
fn panes_are_reached_by_their_words(here: &Here) {
    let (p1, p2) = (callsign_of(here, "p1"), callsign_of(here, "p2"));

    ok(&here.muster(&["pane", "send", "--pane", &p1, "--enter", "--confirm", "echo by-word"]));
    until_text(&mut here.daemon.connect(), "p1", "by-word\n");
    let read = ok(&here.muster(&["pane", "read", "--pane", &p1]));
    assert!(read.contains("echo by-word"), "{read}");
    let waited = ok(&here.muster(&["pane", "wait", "--pane", &p2, "--until", "idle,done"]));
    assert!(waited.starts_with("p2  "), "the wait answers for the pane by its id: {waited}");

    let listed: Value = serde_json::from_str(&ok(&here.muster(&["--json", "window"]))).unwrap();
    let row = listed["panes"].as_array().unwrap().iter().find(|pane| pane["pane"] == json!("p2"));
    let row = row.unwrap_or_else(|| panic!("p2 is listed: {listed}"));
    assert_eq!(row["name"], json!(p2), "{listed}");
    assert!(row["label"].as_str().is_some_and(|label| label.starts_with(&p2)), "{listed}");
}

/// The word `pane`'s daemon drew it.
fn callsign_of(here: &Here, pane: &str) -> String {
    let held = snapshot(&mut here.daemon.connect());
    let record = held.panes.into_iter().find(|record| record.pane == pane);
    record.and_then(|record| record.callsign).unwrap_or_else(|| panic!("{pane} has no callsign"))
}

/// With no window, `--layout` draws each tab from the daemon's own tree, places each pane in it as
/// a window would, and asks the daemon for the sizes.
fn the_layout_is_the_daemons_trees(here: &Here) {
    let laid: Value =
        serde_json::from_str(&ok(&here.muster(&["--json", "window", "--layout"]))).unwrap();
    let layout = &laid["tabs"][0]["regions"][0]["layout"];
    assert_eq!(layout["axis"], json!("columns"), "p2 and p3 went to p1's right: {laid}");
    let mut widths = 0.0;
    for pane in laid["panes"].as_array().into_iter().flatten() {
        assert!(pane["cells"]["cols"].as_u64().is_some_and(|cols| cols > 0), "{pane}");
        assert_eq!(
            pane["frame"]["height"],
            json!(1.0),
            "side by side, each the full height: {pane}"
        );
        widths += pane["frame"]["width"].as_f64().unwrap_or_default();
    }
    assert!((widths - 1.0).abs() < 1e-4, "the three frames share the tab's width: {laid}");
    let drawn = ok(&here.muster(&["window", "--layout"]));
    let p3 = callsign_of(here, "p3");
    assert!(drawn.contains('┬') && drawn.contains(&p3), "three panes side by side:\n{drawn}");
}

/// A wait for a state the pane is already in ends at once; one for a state it reaches later ends
/// when it does, naming the pane; and `idle` is met by the `done` a finish leaves.
fn a_wait_ends_when_the_agent_gets_there(here: &Here) {
    assert_eq!(ok(&here.muster(&["pane", "wait", "--pane", "p2", "--until", "idle"])), "p2  idle");

    let wait =
        here.spawned(&["pane", "wait", "--pane", "p2", "--until", "working", "--timeout", "60"]);
    here.daemon.set_agent_state("p2", proto::AgentState::Working);
    let waited = wait.wait_with_output().unwrap();
    assert_eq!(ok(&waited), "p2  working");

    let wait =
        here.spawned(&["pane", "wait", "--pane", "p2", "--until", "idle", "--timeout", "60"]);
    here.daemon.set_agent_state("p2", proto::AgentState::Idle);
    let waited = ok(&wait.wait_with_output().unwrap());
    assert!(
        waited == "p2  done" || waited == "p2  idle",
        "an agent that finished with nobody looking is done, which a wait for idle accepts: {waited}"
    );

    let ran =
        here.muster(&["pane", "wait", "--pane", "p2", "--until", "blocked", "--timeout", "1"]);
    refused_with(&ran, 5);
}

/// A wait on context says at once that the pane has not said its context, ends when the agent
/// says it is that full, and prints how full beside the state.
fn a_wait_on_context_ends_when_the_agent_says_it(here: &Here) {
    let ran = here.muster(&["pane", "wait", "--pane", "p2", "--context", "80", "--timeout", "1"]);
    let complaint = refused_with(&ran, 5);
    assert!(
        complaint.contains("p2's adapter is not reporting"),
        "a wait its harness may never meet has to say so before the timeout does, and an agent \
         that ended a turn without reporting is one whose adapter is missing:\n{complaint}"
    );
    assert!(complaint.contains("not at 80% context within 1s"), "{complaint}");
    let by_word = callsign_of(here, "p2");
    let ran =
        here.muster(&["pane", "wait", "--pane", &by_word, "--context", "80", "--timeout", "1"]);
    let complaint = refused_with(&ran, 5);
    assert!(
        complaint.contains("p2's adapter is not reporting"),
        "the pane named by its word has the same advice as by its id:\n{complaint}"
    );

    let wait =
        here.spawned(&["pane", "wait", "--pane", "p2", "--context", "80", "--timeout", "60"]);
    let report = proto::pane_request::Report {
        pane: "p2".to_string(),
        context_used: Some(85.0),
        ..Default::default()
    };
    expect(
        &mut here.daemon.connect(),
        pane(proto::pane_request::Request::Report(report)),
        proto::Outcome::Done,
    );
    // `done`: the wait before this one left a finish nobody has looked at.
    assert_eq!(ok(&wait.wait_with_output().unwrap()), "p2  done  85% context");
}

/// With no window, `pane compact` asks the daemon, and a shell, which has no context, is refused.
fn a_compaction_is_asked_of_the_daemon(here: &Here) {
    ok(&here.muster(&["pane", "compact", "--pane", "p3"]));
    let heard = here.daemon.root().join("home/fake-agent-heard");
    until_some("the agent in p3 to read the compaction", || {
        std::fs::read_to_string(&heard).ok()?.lines().any(|line| line == "/compact").then_some(())
    });
    let refused = refused_with(&here.muster(&["pane", "compact", "--pane", "p1"]), 1);
    assert!(refused.contains("runs no agent"), "{refused}");
    a_compaction_queued_says_what_it_waits_for(here);
}

/// A compaction asked of an agent at work says what it waits for, and `muster window` shows it
/// queued until it is typed; the prompt it was asked to carry on with is typed after it.
fn a_compaction_queued_says_what_it_waits_for(here: &Here) {
    here.daemon.set_agent_state("p3", proto::AgentState::Working);
    let asked = ok(&here.muster(&[
        "pane", "compact", "--pane", "p3", "--then", "carry on", "keep", "the", "notes",
    ]));
    assert_eq!(asked, "queued, waiting for its turn to end");
    let listed = ok(&here.muster(&["window"]));
    assert!(listed.contains("(compaction queued, waiting for its turn to end)"), "{listed}");
    let listed: Value = serde_json::from_str(&ok(&here.muster(&["--json", "window"]))).unwrap();
    let p3 = listed["panes"].as_array().unwrap().iter().find(|pane| pane["pane"] == json!("p3"));
    assert_eq!(p3.map(|p3| &p3["compaction"]), Some(&json!("queued")), "{listed}");

    here.daemon.set_agent_state("p3", proto::AgentState::Idle);
    let heard = here.daemon.root().join("home/fake-agent-heard");
    until_some("the agent in p3 to read the compaction and then its prompt", || {
        let heard = std::fs::read_to_string(&heard).ok()?;
        let lines: Vec<&str> = heard.lines().collect();
        let compacted = lines.iter().position(|line| *line == "/compact keep the notes")?;
        lines[compacted..].contains(&"carry on").then_some(())
    });
    until_some("p3 to have nothing queued", || {
        let listed: Value = serde_json::from_str(&ok(&here.muster(&["--json", "window"]))).ok()?;
        let p3 = listed["panes"].as_array()?.iter().find(|pane| pane["pane"] == json!("p3"))?;
        p3["compaction"].is_null().then_some(())
    });
}

/// With no window, `pane restart` asks the daemon, which says what it typed to resume the session,
/// and a shell, which has no session, is refused.
fn an_agent_is_restarted_by_the_daemon(here: &Here) {
    let report = proto::pane_request::Report {
        pane: "p2".to_string(),
        agent: "claude".to_string(),
        session_id: Some("s-9".to_string()),
        ..Default::default()
    };
    expect(
        &mut here.daemon.connect(),
        pane(proto::pane_request::Request::Report(report)),
        proto::Outcome::Done,
    );
    let said = ok(&here.muster(&["pane", "restart", "--pane", "p2", "--env", "FAKE=1"]));
    assert!(
        said.starts_with("p2  ended claude and typed: FAKE='1' ")
            && said.ends_with("'--resume' 's-9'"),
        "{said}"
    );
    here.daemon.until_agent("p2", proto::AgentState::Idle);
    let refused = refused_with(&here.muster(&["pane", "restart", "--pane", "p1"]), 1);
    assert!(refused.contains("nothing to resume"), "{refused}");
}

/// A pane is hidden, made quiet and shown again with no window, by the daemon holding it, which
/// is what lets a machine keep a hidden pane while nothing is attached to it.
fn hiding_and_quiet_are_asked_of_the_daemon(here: &Here) {
    let mut control = here.daemon.connect();
    let tab_of = |control: &mut _, pane: &str| {
        let held = snapshot(control);
        held.tabs
            .into_iter()
            .find(|tab| tab_shape_holds(tab, pane))
            .unwrap_or_else(|| panic!("{pane} is in no tab"))
    };
    ok(&here.muster(&["pane", "hide", "--pane", "p2"]));
    assert!(tab_of(&mut control, "p2").hidden, "p2's tab is not hidden");
    let listed = ok(&here.muster(&["window"]));
    let hidden_from = listed.lines().position(|line| line == "hidden");
    let p2 = callsign_of(here, "p2");
    let p2_at = listed.lines().position(|line| line.contains(&p2));
    assert!(
        matches!((hidden_from, p2_at), (Some(heading), Some(row)) if row > heading),
        "p2 is not listed under `hidden`:\n{listed}"
    );

    ok(&here.muster(&["pane", "quiet", "--pane", "p2"]));
    let quiet = |control: &mut _| {
        snapshot(control)
            .panes
            .into_iter()
            .find(|pane| pane.pane == "p2")
            .is_some_and(|pane| pane.quiet)
    };
    assert!(quiet(&mut control), "p2 is not quiet");
    assert!(
        ok(&here.muster(&["window"]))
            .lines()
            .any(|line| line.contains(&p2) && line.contains("(quiet)"))
    );
    ok(&here.muster(&["pane", "quiet", "--pane", "p2", "--off"]));
    assert!(!quiet(&mut control), "p2 is still quiet");

    ok(&here.muster(&["pane", "show", "--pane", "p2"]));
    assert!(!tab_of(&mut control, "p2").hidden, "p2's tab is still hidden");
    // Showing a pane that is not hidden is already done, and says so as a success.
    ok(&here.muster(&["pane", "show", "--pane", "p2"]));
}

/// A pane is made, named and closed with no window, by the daemon holding the panes, which is what
/// lets an agent on a devenv start and retire workers while the laptop holding its window is gone.
fn panes_are_made_renamed_and_closed_by_the_daemon(here: &Here) {
    let mut control = here.daemon.connect();
    // The pane is told the window the caller was, so it drives that window once one is back.
    let told = format!("test \"$MUSTER_SOCKET\" = '{}' && echo window-told", here.window);
    let made = ok(&here
        .muster(&["pane", "new", "--pane", "p1", "--down", "--name", "worker", "--run", &told]));
    until_text(&mut control, &made, "window-told");
    let held = snapshot(&mut control);
    let record = held.panes.iter().find(|pane| pane.pane == made);
    let record = record.unwrap_or_else(|| panic!("the daemon holds no {made}: {held:?}"));
    assert_eq!(record.label.as_deref(), Some("worker"));
    let tab = held.tabs.iter().find(|tab| tab_shape_holds(tab, &made)).expect("made in a tab");
    assert!(
        tab.tab == "t1" && tab_shape_holds(tab, "p1") && !tab.hidden,
        "a split of p1 lands in p1's tab: {tab:?}"
    );

    let hidden = ok(&here.muster(&["pane", "new", "--pane", "p1", "--hidden"]));
    let held = snapshot(&mut control);
    let tab = held.tabs.iter().find(|tab| tab_shape_holds(tab, &hidden)).expect("made in a tab");
    assert!(tab.hidden && !tab_shape_holds(tab, "p1"), "a hidden tab of its own: {tab:?}");

    // Outside every pane, with no pane named, there is no keyboard's pane to split.
    let alone: Value = serde_json::from_str(&ok(&here.muster(&["--json", "pane", "new"]))).unwrap();
    let alone = alone["pane"].as_str().unwrap_or_else(|| panic!("a pane: {alone}")).to_string();
    let held = snapshot(&mut control);
    let tab = held.tabs.iter().find(|tab| tab_shape_holds(tab, &alone)).expect("made in a tab");
    assert!(tab.tab != "t1" && !tab.hidden, "a tab of its own, shown: {tab:?}");

    ok(&here.muster(&["pane", "rename", "--pane", "worker", "  lead  "]));
    let label = |control: &mut _| {
        snapshot(control)
            .panes
            .into_iter()
            .find(|pane| pane.pane == made)
            .and_then(|pane| pane.label)
    };
    assert_eq!(label(&mut control).as_deref(), Some("lead"), "renamed, trimmed, by its name");
    // Its callsign still reaches it, renamed as it is.
    let callsign = callsign_of(here, &made);
    ok(&here.muster(&["pane", "rename", "--pane", &callsign, "lead again"]));
    assert_eq!(label(&mut control).as_deref(), Some("lead again"), "renamed by its callsign");
    ok(&here.muster(&["pane", "rename", "--pane", &made, ""]));
    assert_eq!(label(&mut control), None, "a blank name takes the name away");

    for pane in [&made, &hidden, &alone] {
        ok(&here.muster(&["pane", "close", "--pane", pane]));
    }
    let left = snapshot(&mut control);
    assert!(
        !left.panes.iter().any(|pane| [&made, &hidden, &alone].contains(&&pane.pane)),
        "all three closed: {left:?}"
    );
    let refused = refused_with(&here.muster(&["pane", "close", "--pane", "p9nobody"]), 1);
    assert!(refused.contains("no pane called p9nobody"), "{refused}");
    let refused = refused_with(&here.muster(&["pane", "rename", "--pane", "p9nobody", "x"]), 1);
    assert!(refused.contains("no pane called p9nobody"), "{refused}");
}

/// A pane's helpers are made with no window as with one: hidden, in one tab named for their maker,
/// listed as its own, and shown and hidden together.
fn helpers_are_made_by_the_daemon(here: &Here) {
    let mut control = here.daemon.connect();
    let first = ok(&here.muster(&["pane", "new", "--helper-of", "p1"]));
    let second = ok(&here.muster(&["pane", "new", "--helper-of", "p1", "--notify"]));
    let held = snapshot(&mut control);
    let maker = held.panes.iter().find(|pane| pane.pane == "p1").expect("p1 is held");
    let tab = held.tabs.iter().find(|tab| tab_shape_holds(tab, &first)).expect("made in a tab");
    assert!(tab.hidden && tab_shape_holds(tab, &second), "one hidden tab for both: {tab:?}");
    let label = tab.label.as_ref().and_then(|label| label.text.clone());
    assert_eq!(label, Some(format!("{}'s helpers", maker.name().unwrap())), "{tab:?}");
    for (helper, quiet) in [(&first, true), (&second, false)] {
        let record = held.panes.iter().find(|pane| &pane.pane == helper).expect("held");
        assert_eq!(record.maker.as_deref(), Some("p1"), "{record:?}");
        assert_eq!(record.quiet, quiet, "quiet unless --notify: {record:?}");
    }

    let window: Value = serde_json::from_str(&ok(&here.muster(&["window", "--json"]))).unwrap();
    let makers: Vec<&Value> = window["hidden"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|tab| tab["panes"].as_array().into_iter().flatten())
        .filter(|pane| pane["pane"] == json!(first) || pane["pane"] == json!(second))
        .map(|pane| &pane["maker"])
        .collect();
    assert_eq!(makers, [&json!("p1"), &json!("p1")], "{window}");
    // Read as a person reads it, each is listed under its maker by its word, as a window lists it.
    let readable = ok(&here.muster(&["window"]));
    for helper in [&first, &second] {
        let record = held.panes.iter().find(|pane| &pane.pane == helper).expect("held");
        let word = record.callsign.as_deref().expect("a helper has a word");
        let row = readable.lines().find(|row| row.trim_start().starts_with(&format!("{word} ")));
        assert!(row.is_some_and(|row| row.starts_with("         ")), "{word}:\n{readable}");
    }

    // Shown together and hidden again, the tab whole.
    let helpers_tab = |control: &mut _| {
        snapshot(control).tabs.into_iter().find(|tab| tab_shape_holds(tab, &first)).expect("held")
    };
    ok(&here.muster(&["pane", "show", "--helpers-of", "p1"]));
    let shown = helpers_tab(&mut control);
    assert!(!shown.hidden && tab_shape_holds(&shown, &second), "{shown:?}");
    ok(&here.muster(&["pane", "hide", "--helpers-of", "p1"]));
    assert!(helpers_tab(&mut control).hidden);
    // Showing one helper shows the tab they share, with the other still in it.
    ok(&here.muster(&["pane", "show", "--pane", &first]));
    let shown = helpers_tab(&mut control);
    assert!(!shown.hidden && tab_shape_holds(&shown, &second), "{shown:?}");
    ok(&here.muster(&["pane", "hide", "--helpers-of", "p1"]));
    let refused = refused_with(&here.muster(&["pane", "show", "--helpers-of", "p2"]), 1);
    assert!(refused.contains("no helper of p2"), "{refused}");

    let refused = refused_with(&here.muster(&["pane", "new", "--helper-of", "p9nobody"]), 1);
    assert!(refused.contains("no pane called p9nobody"), "{refused}");
    for pane in [&first, &second] {
        ok(&here.muster(&["pane", "close", "--pane", pane]));
    }
}

fn tab_shape_holds(tab: &proto::Tab, pane: &str) -> bool {
    fn holds(node: &proto::Node, pane: &str) -> bool {
        match node.node.as_ref() {
            Some(proto::node::Node::Pane(held)) => held == pane,
            Some(proto::node::Node::Split(split)) => {
                split.first.as_ref().is_some_and(|first| holds(first, pane))
                    || split.second.as_ref().is_some_and(|second| holds(second, pane))
            }
            None => false,
        }
    }
    tab.root.as_ref().is_some_and(|root| holds(root, pane))
}

/// `window --watch --json` says each pane as it stands, then each change.
fn a_watch_prints_each_change(here: &Here) {
    let mut watch = here.spawned(&["--json", "window", "--watch"]);
    let lines = lines_of(&mut watch);
    let mut first = Vec::new();
    for _ in 0..3 {
        first.push(next_json(&lines, "a pane as it stands")["pane"].clone());
    }
    first.sort_by_key(ToString::to_string);
    assert_eq!(first, [json!("p1"), json!("p2"), json!("p3")]);

    here.daemon.set_agent_state("p3", proto::AgentState::Working);
    let changed = next_json(&lines, "p3 starting work");
    assert_eq!(
        (&changed["pane"], &changed["state"]),
        (&json!("p3"), &json!("working")),
        "{changed}"
    );
    let _ = watch.kill();
    let _ = watch.wait();
}

/// A wait on a pane that closes can never end, so it fails naming the pane.
fn a_wait_on_a_pane_that_closes_is_refused(here: &Here) {
    // The wait has to be following the daemon before the close, or it is refused as a pane
    // nobody holds: so the close waits for the daemon to log one more subscription.
    let subscribed = r#""request":"session.subscribe""#;
    let mut logging = here.daemon.connect();
    let follow = session_request::Request::FollowLog(session_request::FollowLog { after: None });
    let replayed = match expect(&mut logging, session(follow), proto::Outcome::Done).answer.detail {
        Some(proto::answer::Detail::Followed(followed)) => followed.newest,
        other => panic!("a follow answered with {other:?}"),
    };
    let before = logging
        .logged_through(replayed, PATIENCE)
        .iter()
        .filter(|line| line.line.contains(subscribed))
        .count();
    let wait =
        here.spawned(&["pane", "wait", "--pane", "p3", "--until", "blocked", "--timeout", "60"]);
    logging.logged_times_until(subscribed, before + 1, PATIENCE);
    let mut control = here.daemon.connect();
    expect(&mut control, close_request("p3"), proto::Outcome::Done);
    let ended = wait.wait_with_output().unwrap();
    let refused = refused_with(&ended, 1);
    assert!(refused.contains("pane p3 closed before it was blocked"), "{refused}");
}

/// An agent whose program stops reading gets nowhere until it reads again, so a wait for it to
/// ask for somebody ends refused, naming why; `muster window` says it is stuck and what it left; and a wait
/// for `stuck` is answered.
fn a_wait_on_an_agent_that_stops_reading_is_refused(here: &Here) {
    ok(&here.muster(&["pane", "send", "--pane", "p2", "--enter", "deaf 30"]));
    // Read, and so the evidence that this program reads, before anything waits unread.
    std::thread::sleep(std::time::Duration::from_secs(2));
    let wait =
        here.spawned(&["pane", "wait", "--pane", "p2", "--until", "blocked", "--timeout", "60"]);
    ok(&here.muster(&["pane", "send", "--pane", "p2", "--enter", "are you there"]));
    let ended = wait.wait_with_output().unwrap();
    let refused = refused_with(&ended, 1);
    assert!(
        refused.contains("pane p2 is stuck") && refused.contains("stopped reading its input"),
        "{refused}"
    );
    assert_eq!(
        ok(&here.muster(&["pane", "wait", "--pane", "p2", "--until", "stuck"])),
        "p2  stuck"
    );
    let listed: Value = serde_json::from_str(&ok(&here.muster(&["--json", "window"]))).unwrap();
    let p2 = listed["panes"].as_array().unwrap().iter().find(|pane| pane["pane"] == json!("p2"));
    let p2 = p2.unwrap_or_else(|| panic!("p2 is listed: {listed}"));
    assert_eq!(p2["state"], json!("stuck"), "{listed}");
    assert_eq!(p2["unread_input"]["bytes"], json!(14), "the line sent, and its Return: {listed}");
}

/// A line over 1024 bytes into `cat`, which reads in canonical mode, is never delivered, and a
/// confirmed send says so with the window's own words rather than exiting 0.
fn a_send_the_pane_never_shows_is_refused(here: &Here) {
    ok(&here.muster(&["pane", "send", "--pane", "p1", "--enter", "cat"]));
    let mut control = here.daemon.connect();
    until_text(&mut control, "p1", "cat\n");
    let long = format!("{}end", "x".repeat(1100));
    let ran = here.muster(&["pane", "send", "--pane", "p1", "--enter", "--confirm", &long]);
    let refused = refused_with(&ran, 1);
    assert!(
        refused.contains("only its start shows") && refused.contains("canonical mode"),
        "{refused}"
    );

    // The terminal holds what fit of that line and drops every byte after it, a Return included,
    // so `cat` never gets a line and nothing sent to p1 shows until Ctrl-C clears it.
    let ran = here.muster(&["pane", "send", "--pane", "p1", "--enter", "--confirm", "echo held"]);
    refused_with(&ran, 1);
    ok(&here.muster(&["pane", "send", "--pane", "p1", "--key", "ctrl+c"]));
    ok(&here.muster(&["pane", "send", "--pane", "p1", "--enter", "--confirm", "echo cleared"]));
    until_text(&mut control, "p1", "cleared\n");
}

/// Text sent into an agent's prompt that already holds a draft is refused, naming the draft,
/// since the next Return would submit both as one prompt. `--append` adds to it, and Return
/// alone, or a lone space with it, still submits what the prompt holds.
fn a_send_into_a_prompt_holding_a_draft_is_refused(here: &Here) {
    ok(&here.muster(&["pane", "send", "--pane", "p3", "half typed"]));
    let mut control = here.daemon.connect();
    until_text(&mut control, "p3", "PROBE-PROMPT> half typed");
    let refused = until_some("a send into the draft to be refused", || {
        let ran = here.muster(&["pane", "send", "--pane", "p3", "--enter", "/clear"]);
        (ran.status.code() == Some(1)).then(|| complained(&ran))
    });
    assert!(refused.contains("already holds \"half typed\""), "{refused}");
    assert!(refused.contains("--append"), "{refused}");

    ok(&here.muster(&["pane", "send", "--pane", "p3", "--append", " and more"]));
    until_text(&mut control, "p3", "PROBE-PROMPT> half typed and more");
    ok(&here.muster(&["pane", "send", "--pane", "p3", " ", "--enter"]));
    let heard = here.daemon.root().join("home/fake-agent-heard");
    until_some("the agent in p3 to read the draft it was sent with", || {
        let heard = std::fs::read_to_string(&heard).ok()?;
        heard.lines().any(|line| line == "half typed and more ").then_some(())
    });
    let heard = std::fs::read_to_string(&heard).unwrap();
    assert!(!heard.contains("/clear"), "the refused send was typed: {heard}");
}

/// The agent in p2 takes `working` and repaints, so nothing of what it took shows, which exits 4
/// as the window's answer does: it may have been taken, so sending it again is not safe.
fn a_line_the_agent_took_and_repainted_is_unanswered(here: &Here) {
    let ran = here.muster(&["pane", "send", "--pane", "p2", "--enter", "--confirm", "working"]);
    let unknown = refused_with(&ran, 4);
    assert!(unknown.contains("cannot be told from the screen"), "{unknown}");
    here.daemon.until_agent("p2", proto::AgentState::Working);
    here.daemon.set_agent_state("p2", proto::AgentState::Idle);
}

/// Keys go to the daemon as the window sends them, and a Return pressed alone is a send of its
/// own. A key nobody knows is refused before anything is sent.
///
/// At p1's shell, which keeps the line it ran on screen. The agent in p2 repaints over a line
/// the moment it reads it, so the text would show only for the 50ms before the Return, and a
/// confirmation that reads after that cannot tell whether it was taken.
fn keys_are_pressed_without_a_window(here: &Here) {
    ok(&here.muster(&[
        "pane",
        "send",
        "--pane",
        "p1",
        "--key",
        "enter",
        "--confirm",
        "echo keyed",
    ]));
    let mut control = here.daemon.connect();
    // The output's own row, which only the Return running the command prints.
    until_text(&mut control, "p1", "\nkeyed\n");
    ok(&here.muster(&["pane", "send", "--pane", "p1", "--enter", "--confirm"]));
    let refused =
        refused_with(&here.muster(&["pane", "send", "--pane", "p1", "--key", "hyper"]), 1);
    assert!(refused.contains("nothing was sent"), "{refused}");
}

#[test]
fn only_a_missing_window_is_answered_by_the_daemon() {
    let here = Here::new(Daemon::start_built());
    let mut control = here.daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));

    let refused = refused_with(&here.muster(&["tab", "new"]), 3);
    assert!(
        refused.contains("pane verbs") && refused.contains("still work"),
        "a verb only a window can carry out says what works without one:\n{refused}"
    );
    // Which machine a window calls `elsewhere` is the window's to know.
    let refused = refused_with(&here.muster(&["pane", "new", "--daemon", "elsewhere"]), 3);
    assert!(refused.contains("still work"), "{refused}");

    let named = here.muster(&["--socket", &here.window, "pane", "read", "--pane", "p1"]);
    let refused = refused_with(&named, 3);
    assert!(
        refused.contains("--socket said"),
        "a window the caller named is not passed over for the daemon:\n{refused}"
    );

    let refused = refused_with(&here.muster(&["--no-window", "tab", "new"]), 1);
    assert!(refused.contains("needs a window"), "{refused}");
}

/// A window that takes the connection and never answers - a devenv's forward of a laptop that has
/// gone to sleep - is passed over for the daemon as a missing one is, and the answer says so
/// (kan a_2ZNnSyiXR). A verb only a window carries out is refused just as soon, with nothing sent.
#[test]
fn a_window_that_never_answers_is_answered_for_by_the_daemon() {
    let here = Here::new(Daemon::start_built());
    let listener = std::os::unix::net::UnixListener::bind(&here.window).unwrap();
    std::thread::spawn(move || {
        // Held, not read: each connection stays open and nothing ever comes back.
        let mut held = Vec::new();
        for stream in listener.incoming() {
            held.push(stream);
        }
    });
    let mut control = here.daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");

    let started = std::time::Instant::now();
    let sent = here.muster(&["pane", "send", "--pane", "p1", "--enter", "echo while-asleep"]);
    ok(&sent);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "a send waited {:?} on a window that never answers",
        started.elapsed()
    );
    let note = complained(&sent);
    assert!(
        note.contains(&here.window) && note.contains("muster-daemon answered instead"),
        "the send does not say the daemon answered in place of the silent window:\n{note}"
    );
    until_text(&mut control, "p1", "while-asleep\n");

    let read = here.muster(&["pane", "read", "--pane", "p1"]);
    assert!(ok(&read).contains("echo while-asleep"), "{}", said(&read));
    let listed = ok(&here.muster(&["window"]));
    assert!(listed.starts_with("no window answered; the muster-daemon at"), "{listed}");

    let started = std::time::Instant::now();
    let made = ok(&here.muster(&["pane", "new", "--pane", "p1"]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "a pane made while the window sleeps waited {:?}",
        started.elapsed()
    );
    ok(&here.muster(&["pane", "close", "--pane", &made]));

    let refused = refused_with(&here.muster(&["tab", "new"]), 3);
    assert!(
        refused.contains("did not answer") && refused.contains("still work"),
        "a verb only a window carries out does not say the window was silent:\n{refused}"
    );
}

/// With a window open, `--no-window` answers from the daemon what the window answers.
#[test]
fn the_daemon_and_the_window_say_the_same_about_a_pane() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_detecting();
    let socket = daemon.root().join("command.sock").to_string_lossy().into_owned();
    accepted(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.clone(),
        ..Startup::default()
    })));
    accepted(&dispatch(request::Payload::OpenWindow(OpenWindow::default())));
    let mut here = Here::new(daemon);
    here.window = socket;

    let pane = until_some("the window to describe the pane the daemon holds", || {
        let window: Value =
            serde_json::from_slice(&here.muster(&["--json", "window"]).stdout).ok()?;
        Some(window["panes"].get(0)?["pane"].as_str()?.to_string())
    });
    let mut control = here.daemon.connect();
    until_text(&mut control, &pane, "$");
    ok(&here.muster(&["pane", "send", "--pane", &pane, "--enter", "--confirm", "echo both-ways"]));
    until_text(&mut control, &pane, "both-ways\n");

    for asked in [
        &["--json", "pane", "read", "--pane", &pane][..],
        &["--json", "pane", "read", "--pane", &pane, "--rows", "2"],
    ] {
        let through_the_window = ok(&here.muster(asked));
        let from_the_daemon = ok(&here.muster(&[&["--no-window"][..], asked].concat()));
        assert_eq!(through_the_window, from_the_daemon, "{asked:?}");
    }

    let windowed: Value = serde_json::from_str(&ok(&here.muster(&["--json", "window"]))).unwrap();
    let windowless: Value =
        serde_json::from_str(&ok(&here.muster(&["--no-window", "--json", "window"]))).unwrap();
    let described = |window: &Value| -> Vec<(Value, Value, Value)> {
        let mut panes: Vec<_> = window["panes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pane| (pane["pane"].clone(), pane["tab"].clone(), pane["state"].clone()))
            .collect();
        panes.sort_by_key(|pane| pane.0.to_string());
        panes
    };
    assert_eq!(described(&windowed), described(&windowless), "{windowed}\n{windowless}");
    assert!(windowed.get("answered_by").is_none(), "a window's answer never says it: {windowed}");
}

fn dispatch(payload: request::Payload) -> Response {
    let reply = muster::dispatch(&Request::new(payload).encode_to_vec());
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn accepted(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}

/// A child's stdout, a line at a time as it is written.
fn lines_of(child: &mut Child) -> Receiver<String> {
    let stdout = child.stdout.take().expect("stdout was asked to be piped");
    let (sender, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                return;
            }
        }
    });
    lines
}

fn next_json(lines: &Receiver<String>, what: &str) -> Value {
    let line = lines
        .recv_timeout(PATIENCE)
        .unwrap_or_else(|_| panic!("timed out waiting for {what}: nothing reached the pipe"));
    serde_json::from_str(&line)
        .unwrap_or_else(|error| panic!("a watch line under --json is not JSON ({error}): {line:?}"))
}

/// A daemon that takes a request to make a pane and hangs up may have made it, under the id the
/// CLI minted and sent: the CLI says which, as a window does, rather than invite a second one.
#[test]
fn a_pane_new_the_daemon_never_answers_names_the_pane_it_may_have_made() {
    use std::os::unix::net::UnixListener;
    let root = std::env::temp_dir().join(format!("muster-hangs-up-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("home/state")).unwrap();
    let socket = root.join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = proto::connection::receive::<proto::Hello>(&mut stream);
            let welcome =
                proto::Welcome { protocol: Some(proto::version::PROTOCOL), ..Default::default() };
            let answer =
                proto::HelloAnswer { answer: Some(proto::hello_answer::Answer::Welcome(welcome)) };
            let _ = proto::connection::send(&mut stream, &answer);
            // Takes the request, then hangs up without a word.
            let _ = proto::connection::receive::<proto::Request>(&mut stream);
        }
    });

    let made = Command::new(env!("CARGO_BIN_EXE_muster"))
        .args(["--json", "pane", "new"])
        .env_clear()
        .env("HOME", root.join("home"))
        .env("MUSTER_HOME", root.join("home"))
        .env("MUSTER_SOCKET", root.join("quit.sock"))
        .env("MUSTER_DAEMON_SOCKET", &socket)
        .stdin(Stdio::null())
        .output()
        .expect("the muster binary runs");
    let error: Value = serde_json::from_str(&complained(&made)).unwrap();
    assert_eq!(made.status.code(), Some(4), "{error}");
    let pane = error["pane"].as_str().unwrap_or_else(|| panic!("names the pane: {error}"));
    let said = error["error"].as_str().unwrap_or_default();
    assert!(
        said.contains(&format!("make pane {pane}")) && said.contains("may make a second one"),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
