//! A read of what a pane's agent printed in its last turn: from where the screen stopped reading
//! as it did when the agent went to work, so neither what it printed before nor the line that
//! started the turn comes with it.

use crate::support::*;
use muster_harness::Input;

/// Has the fake agent print `text` above its prompt, and waits until it has: a line typed before
/// that would be echoed where the agent is about to print.
fn say(daemon: &Daemon, control: &mut Control, text: &str) {
    type_line(daemon, &format!("say {text}"));
    until_some(&format!("the agent to print {text:?}"), || {
        let screen = screen_text(control, "p1");
        (screen.contains(text) && !screen.contains("say ")).then_some(())
    });
}

fn type_line(daemon: &Daemon, text: &str) {
    Input::connect(daemon.socket_path()).send(
        "p1",
        proto::input_event::Input::Send(proto::input_event::Send {
            text: text.to_string(),
            enter: true,
        }),
    );
}

#[test]
fn a_turn_reads_what_the_agent_printed_since_it_went_to_work_and_nothing_before() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    say(&daemon, &mut control, "the previous turn's report");

    let unplaced = control.ask(turn_request("p1"));
    assert_eq!(unplaced.outcome(), proto::Outcome::Refused, "a turn read before any turn");
    assert!(unplaced.answer.reason.contains("no turn"), "{}", unplaced.answer.reason);

    // To work without a line typed, so the screen it starts from is the one above.
    daemon.unblock_agent_unasked();
    daemon.until_agent("p1", proto::AgentState::Working);
    say(&daemon, &mut control, "first line of the report");
    say(&daemon, &mut control, "last line of the report");

    let read = expect(&mut control, turn_request("p1"), proto::Outcome::Done);
    let Some(proto::answer::Detail::Text(text)) = read.answer.detail else {
        panic!("a turn read answered {:?}", read.answer)
    };
    let lines: Vec<&str> = text.text.lines().map(str::trim_end).collect();
    assert_eq!(
        lines,
        ["first line of the report", "last line of the report", "PROBE-PROMPT>"],
        "{text:?}"
    );
    assert_eq!(text.turn, Some(text.first_row), "the page starts where the turn did");
}

/// A dialog in the middle of a turn does not start another: what the agent printed before it is
/// still the turn's.
#[test]
fn a_dialog_in_the_middle_of_a_turn_keeps_what_came_before_it() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    daemon.unblock_agent_unasked();
    daemon.until_agent("p1", proto::AgentState::Working);
    say(&daemon, &mut control, "before the dialog");
    daemon.block_agent_unasked();
    daemon.until_agent("p1", proto::AgentState::Blocked);
    daemon.unblock_agent_unasked();
    daemon.until_agent("p1", proto::AgentState::Working);
    say(&daemon, &mut control, "after the dialog");

    let read = expect(&mut control, turn_request("p1"), proto::Outcome::Done);
    let Some(proto::answer::Detail::Text(text)) = read.answer.detail else {
        panic!("a turn read answered {:?}", read.answer)
    };
    assert!(text.text.starts_with("before the dialog\nafter the dialog"), "{text:?}");
}

/// A daemon that took the pane over in a handoff marks the agent's next turn: it starts from the
/// state the daemon before it last published, which its detector does not publish again.
#[test]
fn the_first_turn_after_a_handoff_is_read() {
    let mut daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    say(&daemon, &mut control, "before the handoff");
    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    let mut control = daemon.connect();

    daemon.unblock_agent_unasked();
    daemon.until_agent("p1", proto::AgentState::Working);
    say(&daemon, &mut control, "after the handoff");
    let read = expect(&mut control, turn_request("p1"), proto::Outcome::Done);
    let Some(proto::answer::Detail::Text(text)) = read.answer.detail else {
        panic!("a turn read answered {:?}", read.answer)
    };
    assert!(text.text.starts_with("after the handoff"), "{text:?}");
}
