//! `api.read_pane` (MIP-10 section 4): a pane's text in pages, the pages `muster pane read` reads.

use crate::support::*;
use crate::turn::say;
use api::read_pane::Rows;
use muster_harness::Input;
use proto::{input_event, pane_request};

/// The page the control connection's `pane.read` answers, as the API spells it.
fn read_by_control(control: &mut Control, read: pane_request::Read) -> api::PaneText {
    let asked = expect(control, pane(pane_request::Request::Read(read)), proto::Outcome::Done);
    let Some(proto::answer::Detail::Text(text)) = asked.answer.detail else {
        panic!("a read answered {:?}", asked.answer)
    };
    api::PaneText {
        text: text.text,
        first_row: text.first_row,
        rows: text.rows,
        oldest_row: text.oldest_row,
        end_row: text.total_rows,
        turn_row: text.turn,
        turn_moved: text.turn_moved,
    }
}

fn read(api: &mut Api, pane: &str, rows: Option<Rows>) -> api::PaneText {
    let asked = api.ask(read_pane(pane, rows));
    assert_eq!(asked.outcome(), api::Outcome::Done, "{:?}", asked.answer);
    asked.pane_text().clone()
}

fn rows_from(row: u64, count: u32) -> Rows {
    Rows::From(api::RowsFrom { row, count })
}

/// A pane that has printed 1 to 40, one to a row, and is back at its prompt.
fn counted(daemon: &Daemon, control: &mut Control) {
    make(control, create("p1", in_new_tab("t1")));
    until_text(control, "p1", "$");
    Input::connect(daemon.socket_path()).send(
        "p1",
        input_event::Input::Send(input_event::Send {
            text: "seq 1 40".to_string(),
            enter: true,
            ..Default::default()
        }),
    );
    until_some("p1 to print 40 and a prompt after it", || {
        let text = read_text(control, "p1", 0, 0).text;
        let rows: Vec<&str> = text.lines().map(str::trim_end).collect();
        let at = rows.iter().position(|row| *row == "40")?;
        rows[at + 1..].iter().any(|row| row.contains('$')).then_some(())
    });
}

#[test]
fn a_client_reads_the_pages_muster_pane_read_reads() {
    let daemon = daemon();
    let mut control = daemon.connect();
    counted(&daemon, &mut control);
    let mut api = daemon.api();

    let newest_page = read(&mut api, "p1", None);
    let by_control =
        pane_request::Read { pane: "p1".to_string(), last: u32::MAX, ..Default::default() };
    assert_eq!(newest_page, read_by_control(&mut control, by_control));
    assert!(newest_page.text.contains("\n40\n"), "{newest_page:?}");

    let newest = read(&mut api, "p1", Some(Rows::Newest(5)));
    let by_control = pane_request::Read { pane: "p1".to_string(), last: 5, ..Default::default() };
    assert_eq!(newest, read_by_control(&mut control, by_control));
    let rows: Vec<&str> = newest.text.lines().map(str::trim_end).collect();
    assert_eq!((rows.len(), &rows[..4]), (5, &["37", "38", "39", "40"][..]), "{newest:?}");
    assert!(rows[4].contains('$'), "the newest row is the prompt: {newest:?}");
    assert_eq!(newest.first_row + 5, newest_page.first_row + u64::from(newest_page.rows));

    let first = read(&mut api, "p1", Some(rows_from(newest.first_row, 3)));
    assert_eq!((first.first_row, first.rows), (newest.first_row, 3));
    let next = read(&mut api, "p1", Some(rows_from(first.first_row + u64::from(first.rows), 3)));
    let whole = read_text(&mut control, "p1", newest.first_row, 6).text;
    assert_eq!(format!("{}\n{}", first.text, next.text), whole, "two pages join into one read");
}

#[test]
fn a_turn_reads_what_the_agent_printed_and_is_refused_before_one() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let mut api = daemon.api();
    let turn = || Some(Rows::Turn(api::LastTurn {}));

    let unplaced = api.ask(read_pane("p1", turn()));
    assert_eq!(unplaced.outcome(), api::Outcome::Refused, "a turn read before any turn");
    assert!(unplaced.answer.reason.contains("no turn"), "{}", unplaced.answer.reason);

    daemon.unblock_agent_unasked();
    daemon.until_agent("p1", proto::AgentState::Working);
    say(&daemon, &mut control, "the turn's report");
    let page = read(&mut api, "p1", turn());
    assert_eq!(page, read_by_control(&mut control, turn_read()));
    assert!(page.text.starts_with("the turn's report"), "{page:?}");
    assert_eq!(page.turn_row, Some(page.first_row), "the page starts where the turn did");
}

fn turn_read() -> pane_request::Read {
    pane_request::Read { pane: "p1".to_string(), turn: true, ..Default::default() }
}

#[test]
fn reading_a_pane_this_daemon_does_not_run_is_refused() {
    let daemon = daemon();
    let mut api = daemon.api();
    let refused = api.ask(read_pane("p9", None));
    assert_eq!(refused.outcome(), api::Outcome::Refused);
    assert!(refused.answer.reason.contains("p9"), "{}", refused.answer.reason);
}

/// A pane read is not a pane seen: a finish nobody has looked at stays DONE, and the read fires
/// nothing.
#[test]
fn reading_a_done_pane_leaves_it_done() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    daemon.set_agent_state("p1", proto::AgentState::Working);
    daemon.set_agent_state("p1", proto::AgentState::Idle);
    let mut api = daemon.api();
    let state = |snapshot: &api::Snapshot| {
        snapshot.panes.iter().find(|held| held.pane == "p1").expect("p1").state()
    };
    assert_eq!(state(api.subscribe(&[api::Topic::Panes]).snapshot()), api::PaneState::Done);

    for rows in [None, Some(Rows::Turn(api::LastTurn {}))] {
        let asked = api.ask(read_pane("p1", rows));
        assert_eq!(asked.outcome(), api::Outcome::Done, "{:?}", asked.answer);
        assert!(asked.events.is_empty(), "a read fired {:?}", asked.events);
    }

    let mut again = daemon.api();
    assert_eq!(state(again.subscribe(&[api::Topic::Panes]).snapshot()), api::PaneState::Done);
}
