//! A pane closed by mistake, brought back: under its id, callsign and name, in its directory and
//! its place, running its agent's session again (`PaneRequest.Reopen`).

use crate::persistence::{record, stop, until_restored};
use crate::resume::reported;
use crate::support::*;
use proto::{pane_request, session_request, tab_request};

fn reopen_request(pane: &str, tab: &str) -> proto::request::Service {
    pane_request_of(pane_request::Request::Reopen(pane_request::Reopen {
        pane: pane.to_string(),
        tab: tab.to_string(),
    }))
}

fn pane_request_of(request: pane_request::Request) -> proto::request::Service {
    pane(request)
}

/// Reopens `pane` and insists it came back, saying how.
#[track_caller]
pub(crate) fn reopened(control: &mut Control, pane: &str, tab: &str) -> proto::Reopened {
    match expect(control, reopen_request(pane, tab), proto::Outcome::Done).answer.detail {
        Some(proto::answer::Detail::Reopened(reopened)) => reopened,
        other => panic!("a reopen answered with {other:?}"),
    }
}

fn rename(control: &mut Control, pane: &str, label: &str) {
    let rename = pane_request::Rename { pane: pane.to_string(), label: Some(label.to_string()) };
    expect(control, pane_request_of(pane_request::Request::Rename(rename)), proto::Outcome::Done);
}

pub(crate) fn close(control: &mut Control, pane: &str) {
    expect(control, close_request(pane), proto::Outcome::Done);
}

fn set_reopen_closed(control: &mut Control, minutes: u32) {
    let set = proto::SetReopenClosed { minutes };
    let request = session(session_request::Request::SetReopenClosed(set));
    expect(control, request, proto::Outcome::Done);
}

fn callsign(snapshot: &proto::Snapshot, pane: &str) -> String {
    record(snapshot, pane).callsign.clone().expect("every pane has a callsign")
}

#[test]
fn a_closed_pane_comes_back_under_its_id_callsign_and_name_where_it_was() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let directory = daemon.root().join("work");
    std::fs::create_dir_all(&directory).unwrap();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(
        &mut control,
        pane_request::Create {
            cwd: Some(directory.display().to_string()),
            ..create("p2", beside("p1", proto::Side::Down))
        },
    );
    make(&mut control, create("p3", beside("p1", proto::Side::Right)));
    rename(&mut control, "p2", "builder");
    let shape = tab_shape(&mut control, "t1");
    let otter = callsign(&snapshot(&mut control), "p2");

    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    let closing = expect(&mut control, close_request("p2"), proto::Outcome::Done);
    let events = events_until_from(&mut control, closing.events, "p2 to close", |events| {
        events.iter().any(|event| named(event) == "pane_closed:p2")
    });
    let remembered = events.iter().find_map(|event| match &event.event {
        Some(proto::event::Event::PaneClosed(closed)) if closed.pane == "p2" => {
            closed.remembered.clone()
        }
        _ => None,
    });
    let remembered = remembered.expect("the close says the pane is remembered");
    assert_eq!(remembered.callsign, otter);
    assert_eq!(remembered.label.as_deref(), Some("builder"));
    assert!(remembered.until_ms > remembered.closed_at_ms);
    let listed = snapshot(&mut control).closed;
    assert_eq!(listed.iter().map(|closed| closed.pane.as_str()).collect::<Vec<_>>(), ["p2"]);

    let back = reopened(&mut control, &otter, "t9");

    assert_eq!(back.pane, "p2", "found by its callsign");
    assert_eq!(back.callsign, otter);
    assert_eq!((back.tab.as_str(), back.made_tab, back.resumed), ("t1", false, false));
    assert_eq!(tab_shape(&mut control, "t1"), shape, "back where it was");
    let snapshot = snapshot(&mut control);
    let p2 = record(&snapshot, "p2");
    assert_eq!(p2.label.as_deref(), Some("builder"));
    assert_eq!(canonical(std::path::Path::new(&p2.cwd)), canonical(&directory));
    assert!(snapshot.closed.is_empty(), "a reopened pane is no longer remembered");
    let again = expect(&mut control, reopen_request("p2", "t9"), proto::Outcome::NotThere);
    assert!(again.answer.reason.contains("reopened already"), "{}", again.answer.reason);
}

#[test]
fn a_pane_alone_in_its_tab_comes_back_in_a_new_tab_named_as_its_old_one() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let label = proto::Label { text: Some("work".to_string()), generation: 2 };
    let rename = tab_request::Rename { tab: "t1".to_string(), label: Some(label) };
    expect(&mut control, tab(tab_request::Request::Rename(rename)), proto::Outcome::Done);
    close(&mut control, "p1");

    let back = reopened(&mut control, "p1", "t2");

    assert_eq!((back.tab.as_str(), back.made_tab), ("t2", true));
    let snapshot = snapshot(&mut control);
    let t2 = snapshot.tabs.iter().find(|tab| tab.tab == "t2").expect("the new tab");
    assert_eq!(t2.label.as_ref().and_then(|label| label.text.as_deref()), Some("work"));
}

/// Each pane of a closed tab comes back, in a tab of its own.
#[test]
fn the_panes_of_a_closed_tab_each_come_back() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", proto::Side::Right)));
    let close = tab_request::Close { tab: "t1".to_string() };
    expect(&mut control, tab(tab_request::Request::Close(close)), proto::Outcome::Done);
    assert_eq!(snapshot(&mut control).closed.len(), 2);

    assert!(reopened(&mut control, "p1", "t2").made_tab);
    assert!(reopened(&mut control, "p2", "t3").made_tab);
}

#[test]
fn an_agent_comes_back_in_its_session_with_its_flags() {
    let (daemon, mut control) = reported("--model opus --effort high 'the first prompt'", "s-1");
    close(&mut control, "p1");

    let back = reopened(&mut control, "p1", "t2");

    assert!(back.resumed);
    let command = until_some("p1 to come back running its agent", || {
        record(&snapshot(&mut control), "p1").command.clone()
    });
    assert!(
        command.ends_with("claude --model opus --effort high --resume s-1"),
        "resumed as {command:?}"
    );
    assert_eq!(record(&snapshot(&mut control), "p1").session_id.as_deref(), Some("s-1"));
    daemon.until_agent("p1", proto::AgentState::Idle);
}

/// What ran in the pane is gone before it starts again, even a program that ignores the hang-up
/// a close sends: a resumed session must not start beside the agent still writing to it.
#[test]
fn a_pane_starts_again_only_once_what_ran_in_it_has_ended() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let pid = daemon.root().join("pid");
    make(
        &mut control,
        pane_request::Create {
            command: Some(format!(
                "sh -c 'trap \"\" HUP; echo $$ > {}; exec sleep 60'",
                pid.display()
            )),
            ..create("p1", in_new_tab("t1"))
        },
    );
    let pid = written(&pid);
    close(&mut control, "p1");

    reopened(&mut control, "p1", "t2");

    assert_eq!(process_state(&pid), "", "the program that ignored its hang-up is gone");
}

#[test]
fn a_closed_pane_is_remembered_across_a_restart() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", proto::Side::Right)));
    let otter = callsign(&snapshot(&mut control), "p2");
    close(&mut control, "p2");
    stop(&mut daemon, &mut control);
    daemon.restart();
    let mut control = daemon.connect();
    let restored = until_restored(&mut control, 1);
    assert_eq!(restored.closed.len(), 1);

    let back = reopened(&mut control, "p2", "t9");

    assert_eq!((back.callsign, back.tab.as_str()), (otter, "t1"));
    assert_eq!(tab_shape(&mut control, "t1"), "[p1|p2 0.50]");
}

#[test]
fn nothing_is_remembered_with_the_setting_at_zero_nor_a_pane_whose_program_ended() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(
        &mut control,
        pane_request::Create {
            command: Some("exit".to_string()),
            ..create("p2", in_new_tab("t2"))
        },
    );
    until_some("p2 to end on its own", || {
        snapshot(&mut control).panes.iter().all(|pane| pane.pane != "p2").then_some(())
    });
    set_reopen_closed(&mut control, 0);
    close(&mut control, "p1");

    assert!(snapshot(&mut control).closed.is_empty());
    expect(&mut control, reopen_request("p1", "t3"), proto::Outcome::NotThere);
    expect(&mut control, reopen_request("p2", "t3"), proto::Outcome::NotThere);
}

/// A closed maker and its helper end up joined again whichever is reopened first, the helper
/// quiet again as it was before its maker closed.
#[test]
fn a_maker_and_its_helpers_are_joined_again_in_either_order() {
    for helper_closes_too in [false, true] {
        let daemon = daemon();
        let mut control = daemon.connect();
        make(&mut control, create("m1", in_new_tab("t1")));
        make(
            &mut control,
            pane_request::Create {
                quiet: true,
                ..helper("h1", "m1", among_helpers("t2", "🤖 m1's helpers"))
            },
        );
        close(&mut control, "m1");
        let h1 = record(&snapshot(&mut control), "h1").clone();
        assert!(h1.maker_closed && !h1.quiet, "a helper left alone asks for the person");
        if helper_closes_too {
            close(&mut control, "h1");
        }

        reopened(&mut control, "m1", "t3");
        if helper_closes_too {
            reopened(&mut control, "h1", "t4");
        }

        let snapshot = snapshot(&mut control);
        let h1 = record(&snapshot, "h1");
        assert!(!h1.maker_closed, "joined again (helper closed too: {helper_closes_too})");
        assert!(h1.quiet, "quiet again (helper closed too: {helper_closes_too})");
        let holding = |tab: &&proto::Tab| tab.root.as_ref().is_some_and(|root| shape(root) == "h1");
        let helpers = snapshot.tabs.iter().find(holding);
        assert!(helpers.is_some_and(|tab| tab.hidden), "in a hidden tab of its own: {snapshot:?}");
    }
}

/// A maker closed with its helpers at every depth, deepest first, comes back joined to them
/// whichever end is reopened first, with each reopen sent as soon as the last is answered.
#[test]
fn a_maker_closed_with_its_helpers_deepest_first_comes_back_joined_in_either_order() {
    for maker_first in [true, false] {
        let daemon = daemon();
        let mut control = daemon.connect();
        make(&mut control, create("m1", in_new_tab("t1")));
        make(
            &mut control,
            pane_request::Create {
                quiet: true,
                ..helper("h1", "m1", among_helpers("t2", "🤖 m1's helpers"))
            },
        );
        make(
            &mut control,
            pane_request::Create {
                quiet: true,
                ..helper("h2", "h1", among_helpers("t3", "🤖 h1's helpers"))
            },
        );
        for pane in ["h2", "h1", "m1"] {
            close(&mut control, pane);
        }

        let order = if maker_first { ["m1", "h1", "h2"] } else { ["h2", "h1", "m1"] };
        for (index, pane) in order.into_iter().enumerate() {
            reopened(&mut control, pane, &format!("t{}", 10 + index));
        }

        let snapshot = snapshot(&mut control);
        for (pane, maker) in [("h1", "m1"), ("h2", "h1")] {
            let record = record(&snapshot, pane);
            assert_eq!(record.maker.as_deref(), Some(maker));
            assert!(!record.maker_closed, "{pane} joined again (maker first: {maker_first})");
            assert!(record.quiet, "{pane} quiet again (maker first: {maker_first})");
        }
        assert!(snapshot.closed.is_empty(), "every one reopened: {:?}", snapshot.closed);
    }
}

/// A helper reopened while its maker, closed on the same machine, has not been asks for the person
/// as its maker's close would have left it.
#[test]
fn a_helper_reopened_before_its_maker_asks_for_the_person_until_its_maker_is_back() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("m1", in_new_tab("t1")));
    make(
        &mut control,
        pane_request::Create {
            quiet: true,
            ..helper("h1", "m1", among_helpers("t2", "🤖 m1's helpers"))
        },
    );
    close(&mut control, "h1");
    close(&mut control, "m1");

    reopened(&mut control, "h1", "t3");
    let h1 = record(&snapshot(&mut control), "h1").clone();
    assert!(h1.maker_closed && !h1.quiet, "left as its maker's close leaves a helper: {h1:?}");

    reopened(&mut control, "m1", "t4");
    let h1 = record(&snapshot(&mut control), "h1").clone();
    assert!(!h1.maker_closed && h1.quiet, "taken back by its maker: {h1:?}");
}

/// A callsign another pane took as its name while the pane was closed is not given back, since
/// it would then mean two panes; the pane is drawn another.
#[test]
fn a_pane_whose_callsign_was_taken_meanwhile_comes_back_with_another() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", proto::Side::Right)));
    let otter = callsign(&snapshot(&mut control), "p2");
    close(&mut control, "p2");
    rename(&mut control, "p1", &otter);

    let back = reopened(&mut control, "p2", "t9");

    assert_eq!(back.callsign_taken.as_deref(), Some(otter.as_str()));
    assert_ne!(back.callsign, otter);
    assert_eq!(callsign(&snapshot(&mut control), "p2"), back.callsign);
}

#[test]
fn a_pane_never_closed_here_is_not_there_to_reopen() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let refused = expect(&mut control, reopen_request("nobody", "t2"), proto::Outcome::NotThere);
    assert!(refused.answer.reason.contains("nobody"), "{}", refused.answer.reason);
}

/// A reopen waiting for a program that ignores its hang-up holds up nothing else asked on its
/// connection, and the name the pane is coming back under counts as taken meanwhile: a pane
/// reopened beside it is not given that name as its callsign.
#[test]
fn a_reopen_still_waiting_holds_up_no_request_and_keeps_its_name() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(
        &mut control,
        pane_request::Create {
            command: Some("sh -c 'trap \"\" HUP; exec sleep 60'".to_string()),
            ..create("p1", in_new_tab("t1"))
        },
    );
    make(&mut control, create("p2", beside("p1", proto::Side::Right)));
    let otter = callsign(&snapshot(&mut control), "p2");
    close(&mut control, "p2");
    rename(&mut control, "p1", &otter);
    close(&mut control, "p1");

    let reopening = control.send(reopen_request("p1", "t8"));
    let set = proto::SetReopenClosed { minutes: 60 };
    let behind = control.send(session(session_request::Request::SetReopenClosed(set)));
    let first = next_answer(&mut control);
    let back = reopened(&mut daemon.connect(), "p2", "t9");
    let last = next_answer(&mut control);

    assert_eq!(first.id, behind, "the request sent behind the reopen was answered first");
    assert_eq!((last.id, last.outcome()), (reopening, proto::Outcome::Done), "{}", last.reason);
    assert_eq!(back.callsign_taken.as_deref(), Some(otter.as_str()));
    assert_ne!(back.callsign, otter);
}

/// A handoff passes on what a closed pane's close is still ending, so a reopen on the new daemon
/// waits for it as the old one would have.
#[test]
fn a_reopen_after_a_handoff_waits_for_what_the_pane_ran_to_end() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    let pid = daemon.root().join("pid");
    make(
        &mut control,
        pane_request::Create {
            command: Some(format!(
                "sh -c 'trap \"\" HUP; echo $$ > {}; exec sleep 60'",
                pid.display()
            )),
            ..create("p1", in_new_tab("t1"))
        },
    );
    let pid = written(&pid);
    close(&mut control, "p1");
    let handed = daemon.replace(None);
    assert_eq!(handed.outcome(), proto::Outcome::Done, "{}", handed.reason);

    reopened(&mut daemon.connect(), "p1", "t2");

    assert_eq!(process_state(&pid), "", "the program that ignored its hang-up is gone");
}

/// The next answer on `control`, passing over events.
fn next_answer(control: &mut Control) -> proto::Answer {
    loop {
        match control.next_message(muster_harness::PATIENCE) {
            Some(proto::control_message::Message::Answer(answer)) => return answer,
            Some(_) => {}
            None => panic!("no answer arrived within {:?}", muster_harness::PATIENCE),
        }
    }
}
