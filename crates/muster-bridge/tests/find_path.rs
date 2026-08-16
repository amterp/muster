//! Finding something in a pane, end to end, with a real daemon and a real surface behind it.
//!
//! Every layer of this gesture is covered alone: the matcher by `find.json`, the request by
//! `backend-intent.json`, the row-to-offset alignment by `muster-herdr`'s own daemon tests, and
//! the bar by the Swift suite. None of them says the composition works, and a find that counted
//! correctly and landed on nothing would pass all four - which is what card a_298rp2Pss is
//! about.
//!
//! Here rather than in `muster-seam` because landing needs a pane channel with something on the
//! other end of it. The core scrolls through the same channel a wheel does, so without a bridge
//! the scroll goes to a socket nobody is listening on and the counter is right while the pane
//! never moves - which is exactly the failure worth catching.
//!
//! The oracle is the grid libghostty-vt renders from the frame stream, which is what the person
//! would be looking at.
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use muster::proto::{EndFind, Find, FindStep, request, response};
use serde_json::json;
use support::{Typing, answer, assert_ok, until};

/// Enough rows to be worth searching and few enough to stay inside one read.
const ROWS: u32 = 200;

#[test]
fn a_needle_is_counted_and_the_pane_lands_on_what_it_found() {
    let typing = Typing::start("");

    // One awk rather than a shell loop, and a marker the command's own echo does not carry:
    // `ruler-%05d` holds no `ruler-0`, so every match below is a row the ruler printed.
    let script = format!("awk 'BEGIN{{for(i=1;i<={ROWS};i++) printf \"ruler-%05d\\n\", i}}'\n");
    typing
        .daemon
        .call("pane.send_text", &json!({ "pane_id": typing.pane.as_str(), "text": script }));
    typing.expect_on_screen(
        &format!("ruler-{ROWS:05}"),
        "the ruler never finished printing, so there was nothing to search",
    );

    // Nothing typed is not a search that matched everything, which is what the bar shows the
    // instant it opens.
    let empty = findings(request::Payload::Find(Find::default()));
    assert_eq!(empty.total, 0, "an empty needle matched something");
    assert_eq!(empty.selected, 0, "an empty needle selected something");

    // Every row, so the count is a number this test knows rather than one it reads back.
    let all = findings(find("ruler-0"));
    assert_eq!(all.total, ROWS, "one match per printed row, and none in the command's echo");
    assert_eq!(all.selected, 1, "the first match is selected, counting from one");
    assert!(all.rows_searched >= ROWS, "the read reached every printed row");
    assert!(!all.truncated, "{ROWS} rows is well inside what herdr hands over");

    // One row deep in the pane, and the claim the whole feature rests on: the core reads the
    // history, works out where the match is, and scrolls the pane onto it. Nothing here asks
    // for a scroll - that is what landing means.
    let wanted = "ruler-00042";
    let found = findings(find(wanted));
    assert_eq!(found.total, 1, "exactly one row carries that number");
    until(
        &format!("the pane to land on {wanted}"),
        || typing.bridge.lines().iter().any(|line| line.contains(wanted)),
        || {
            typing.bridge.diagnosis(
                "the match was counted and the pane never scrolled to it, so the counter is \
                 right about a screen showing something else",
            )
        },
    );

    // Walking wraps, because a list somebody is stepping through has no reason to stop at one
    // end of it - and a chord that silently does nothing is worse than one that comes round.
    assert_eq!(findings(find("ruler-0")).selected, 1);
    assert_eq!(findings(step("next")).selected, 2);
    assert_eq!(findings(step("previous")).selected, 1);
    assert_eq!(
        findings(step("previous")).selected,
        ROWS,
        "stepping back from the first did not wrap"
    );

    // Ending it is what closing the bar means, and a step afterwards is refused in words rather
    // than quietly doing nothing.
    assert_ok(&answer(request::Payload::EndFind(EndFind {})));
    let reason = refusal(step("next"));
    assert!(
        reason.contains("nothing is being searched for"),
        "stepping with no search open should say so, and said: {reason}"
    );
}

fn find(needle: &str) -> request::Payload {
    request::Payload::Find(Find { needle: needle.to_string(), ..Find::default() })
}

fn step(direction: &str) -> request::Payload {
    request::Payload::FindStep(FindStep { direction: direction.to_string() })
}

fn findings(payload: request::Payload) -> muster::proto::Findings {
    match answer(payload).payload {
        Some(response::Payload::Findings(findings)) => findings,
        other => panic!("expected findings, got {other:?}"),
    }
}

fn refusal(payload: request::Payload) -> String {
    match answer(payload).payload {
        Some(response::Payload::Failure(failure)) => failure.reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}
