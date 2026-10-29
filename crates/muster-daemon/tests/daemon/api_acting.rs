//! Acting through the API (MIP-10, section 5): `pane.new`, the five outcomes an answer can be,
//! and operation ids, which make asking again after a lost answer safe across a reconnect, a
//! handoff and a restart.

use std::path::Path;

use crate::persistence::{directory, stop};
use crate::support::*;

/// A `pane.new` under operation `id`, named `name`, writing a line to `out` as it starts.
fn writing(id: &str, name: &str, out: &Path) -> api::NewPane {
    api::NewPane {
        operation: id.to_string(),
        run: format!("echo ran >> {}", out.display()),
        name: name.to_string(),
        ..api::NewPane::default()
    }
}

fn made(asked: &ApiAsked) -> api::Made {
    match &asked.answer.detail {
        Some(api::answer::Detail::Made(made)) => made.clone(),
        _ => panic!("the answer names no pane: {:?}", asked.answer),
    }
}

fn answered(tried: Tried) -> ApiAsked {
    match tried {
        Tried::Answered(asked) => asked,
        Tried::Unknown(ended) => panic!("the connection ended before the answer: {ended:?}"),
    }
}

/// The panes the daemon holds named `name`.
fn named_panes(control: &mut Control, name: &str) -> Vec<proto::Pane> {
    snapshot(control).panes.into_iter().filter(|pane| pane.label.as_deref() == Some(name)).collect()
}

fn tab_holding<'a>(held: &'a proto::Snapshot, pane: &str) -> &'a proto::Tab {
    held.tabs
        .iter()
        .find(|tab| tab.root.as_ref().is_some_and(|root| shape(root).contains(pane)))
        .unwrap_or_else(|| panic!("no tab holds {pane}"))
}

/// How many times a pane's command has run, by the lines it wrote.
fn runs(out: &Path) -> usize {
    std::fs::read_to_string(out).map_or(0, |text| text.lines().count())
}

#[test]
fn a_new_pane_runs_its_command_in_its_directory_in_a_tab_of_its_own() {
    let daemon = daemon();
    let work = directory(&daemon, "work");
    let out = daemon.root().join("out");
    let mut api = daemon.api();
    let asked = api.ask(new_pane(api::NewPane {
        operation: "op-1".to_string(),
        run: format!("pwd > {}", out.display()),
        directory: work.display().to_string(),
        name: "builder".to_string(),
        ..api::NewPane::default()
    }));
    assert_eq!(asked.outcome(), api::Outcome::Done, "{:?}", asked.answer);
    let made = made(&asked);

    assert_eq!(written(&out).trim(), work.display().to_string());
    let held = snapshot(&mut daemon.connect());
    let pane = held.panes.iter().find(|pane| pane.pane == made.pane).expect("the pane is held");
    assert_eq!(pane.label.as_deref(), Some("builder"));
    assert_eq!(pane.callsign.as_deref(), Some(made.callsign.as_str()));
    assert!(!pane.quiet);
    let tab = tab_holding(&held, &made.pane);
    assert!(!tab.hidden);
    assert_eq!(shape(tab.root.as_ref().unwrap()), made.pane, "alone in its tab");
}

#[test]
fn a_helper_is_hidden_in_its_makers_helpers_tab_quiet_and_starts_in_its_makers_directory() {
    let daemon = daemon();
    let work = directory(&daemon, "work");
    let mut control = daemon.connect();
    make(
        &mut control,
        proto::pane_request::Create {
            cwd: Some(work.display().to_string()),
            label: Some("lead".to_string()),
            ..create("p0", in_new_tab("t0"))
        },
    );
    let out = daemon.root().join("out");
    let mut api = daemon.api();
    let asked = api.ask(new_pane(api::NewPane {
        helper_of: "p0".to_string(),
        run: format!("pwd > {}", out.display()),
        ..writing("op-1", "", &out)
    }));
    assert_eq!(asked.outcome(), api::Outcome::Done, "{:?}", asked.answer);
    let made = made(&asked);

    assert_eq!(written(&out).trim(), work.display().to_string());
    let held = snapshot(&mut control);
    let pane = held.panes.iter().find(|pane| pane.pane == made.pane).unwrap();
    assert_eq!(pane.maker.as_deref(), Some("p0"));
    assert!(pane.quiet, "a helper is quiet unless asked otherwise");
    let tab = tab_holding(&held, &made.pane);
    assert!(tab.hidden);
    let label = tab.label.as_ref().and_then(|label| label.text.as_deref());
    assert_eq!(label, Some(muster_core::making::helpers_tab("lead").as_str()));
}

#[test]
fn a_new_pane_is_refused_with_a_code_for_what_it_lacks_and_nothing_is_made() {
    let daemon = daemon();
    let out = daemon.root().join("out");
    let mut api = daemon.api();
    let cases = [
        (
            api::NewPane { operation: String::new(), ..writing("", "a", &out) },
            api::Refusal::Invalid,
        ),
        (
            api::NewPane { directory: "relative".to_string(), ..writing("op-1", "a", &out) },
            api::Refusal::Invalid,
        ),
        (
            api::NewPane { machine: "d-elsewhere".to_string(), ..writing("op-2", "a", &out) },
            api::Refusal::NotFound,
        ),
        (
            api::NewPane { helper_of: "p-missing".to_string(), ..writing("op-3", "a", &out) },
            api::Refusal::NotFound,
        ),
    ];
    for (asked, refusal) in cases {
        let answer = api.ask(new_pane(asked.clone()));
        assert_eq!(
            (answer.outcome(), answer.refusal()),
            (api::Outcome::Refused, refusal),
            "{asked:?}"
        );
        assert!(!answer.answer.reason.is_empty());
    }
    assert!(snapshot(&mut daemon.connect()).panes.is_empty());
}

#[test]
fn asking_again_with_the_same_operation_id_makes_no_second_pane_even_after_it_closed() {
    let daemon = daemon();
    let out = daemon.root().join("out");
    let mut control = daemon.connect();
    let mut api = daemon.api();
    let first = api.ask(new_pane(writing("op-1", "once", &out)));
    assert_eq!(first.outcome(), api::Outcome::Done, "{:?}", first.answer);
    written(&out);

    let again = api.ask(new_pane(writing("op-1", "once", &out)));
    assert_eq!(again.outcome(), api::Outcome::AlreadySo, "{:?}", again.answer);
    assert_eq!(made(&again), made(&first));
    assert_eq!(named_panes(&mut control, "once").len(), 1);

    expect(&mut control, close_request(&made(&first).pane), proto::Outcome::Done);
    let after = daemon.api().ask(new_pane(writing("op-1", "once", &out)));
    assert_eq!(after.outcome(), api::Outcome::AlreadySo, "{:?}", after.answer);
    assert_eq!(made(&after), made(&first));
    assert!(named_panes(&mut control, "once").is_empty());
    assert_eq!(runs(&out), 1);
}

#[test]
fn an_operation_id_names_one_request_of_one_client() {
    let daemon = daemon();
    let out = daemon.root().join("out");
    let mut api = daemon.api();
    let first = api.ask(new_pane(writing("op-1", "first", &out)));
    assert_eq!(first.outcome(), api::Outcome::Done, "{:?}", first.answer);

    let other = api.ask(new_pane(writing("op-1", "second", &out)));
    assert_eq!(
        (other.outcome(), other.refusal()),
        (api::Outcome::Refused, api::Refusal::OperationReused)
    );

    let client = api::Client { name: "another".to_string(), version: String::new() };
    let mut another = Api::connect_as(daemon.socket_path(), client);
    let theirs = another.ask(new_pane(writing("op-1", "first", &out)));
    assert_eq!(theirs.outcome(), api::Outcome::Done, "{:?}", theirs.answer);
    assert_ne!(made(&theirs).pane, made(&first).pane);
}

/// The card's proof: a client whose answer was lost asks again on the daemon that took over,
/// under the same id, and the pane is made once.
#[test]
fn a_lost_answer_asked_again_after_a_handoff_makes_one_pane() {
    let mut daemon = daemon();
    let out = daemon.root().join("out");
    daemon.api().send_and_hang_up(new_pane(writing("op-1", "once", &out)));
    let mut control = daemon.connect();
    let pane =
        until_some("the daemon to make the pane", || named_panes(&mut control, "once").pop());

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    let again = answered(daemon.api().try_ask(new_pane(writing("op-1", "once", &out))));
    assert_eq!(again.outcome(), api::Outcome::AlreadySo, "{:?}", again.answer);
    assert_eq!(made(&again).pane, pane.pane);
    assert_eq!(named_panes(&mut daemon.connect(), "once").len(), 1);
    assert_eq!(runs(&out), 1);
}

/// The operations are back before the daemon answers anything, not once its tabs are.
#[test]
fn an_operation_is_remembered_by_a_restarted_daemon_from_its_first_answer() {
    const TABS: usize = 20;
    let mut daemon = daemon();
    let mut control = daemon.connect();
    for n in 0..TABS {
        make(&mut control, create(&format!("p{n}"), in_new_tab(&format!("t{n}"))));
    }
    let out = daemon.root().join("out");
    let first = daemon.api().ask(new_pane(writing("op-1", "once", &out)));
    assert_eq!(first.outcome(), api::Outcome::Done, "{:?}", first.answer);
    stop(&mut daemon, &mut control);
    daemon.restart();

    let mut api = daemon.api();
    let again = api.ask(new_pane(writing("op-1", "once", &out)));
    assert_eq!(again.outcome(), api::Outcome::AlreadySo, "{:?}", again.answer);
    assert_eq!(made(&again), made(&first));
    let sources = api.subscribe(&[]).snapshot().sources.clone();
    assert_eq!(
        sources[0].state(),
        api::SourceState::Restoring,
        "{} tabs take longer than two answers to bring back, so the repeat was answered mid-restore",
        TABS + 1
    );
    let mut control = daemon.connect();
    let restored = until_some("the tabs to come back", || {
        let held = snapshot(&mut control);
        (held.panes.len() == TABS + 1).then_some(held)
    });
    assert_eq!(
        restored.panes.iter().filter(|pane| pane.label.as_deref() == Some("once")).count(),
        1
    );
}

/// A pane that cannot start never ran anything, so it is refused, not unknown, and asking again
/// under the same id once the cause is put right makes it.
#[test]
fn a_pane_that_could_not_start_is_refused_and_can_be_asked_for_again() {
    let daemon = daemon();
    let later = daemon.root().join("later");
    let out = daemon.root().join("out");
    let asked =
        || api::NewPane { directory: later.display().to_string(), ..writing("op-1", "late", &out) };
    let mut api = daemon.api();
    let refused = api.ask(new_pane(asked()));
    assert_eq!(refused.outcome(), api::Outcome::Refused, "{:?}", refused.answer);

    std::fs::create_dir(&later).unwrap();
    let done = api.ask(new_pane(asked()));
    assert_eq!(done.outcome(), api::Outcome::Done, "{:?}", done.answer);
    written(&out);
}

/// A command that started, whose pane was then lost to a handoff, may have done what it does:
/// the client hears unknown, on the connection and from the successor, and it never runs again.
#[test]
fn a_command_that_ran_and_lost_its_pane_is_unknown_and_never_runs_again() {
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_API_STARTED_DELAY", "3000")]);
    let out = daemon.root().join("out");
    let mut api = daemon.api();
    let asked = writing("op-1", "lost", &out);
    let mut writer = api.writer();
    let request =
        api::Request { correlation: "c1".to_string(), request: Some(new_pane(asked.clone())) };
    muster_api::connection::send(&mut writer, &request).unwrap();
    until_some("the command to run", || (runs(&out) == 1).then_some(()));

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    let (ended, before) = api.ended();
    assert_eq!(ended.reason(), api::ended::Reason::Replaced);
    assert!(before.is_empty(), "nothing answered the request: {before:?}");

    let again = answered(daemon.api().try_ask(new_pane(asked)));
    assert_eq!(again.outcome(), api::Outcome::Unknown, "{:?}", again.answer);
    assert!(named_panes(&mut daemon.connect(), "lost").is_empty());
    assert_eq!(runs(&out), 1);
}

/// A request written on a connection that ends before its answer is unknown to the client, never
/// refused.
#[test]
fn a_request_whose_connection_ends_before_its_answer_is_unknown_to_the_client() {
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_API_STARTED_DELAY", "3000")]);
    let out = daemon.root().join("out");
    let mut api = daemon.api();
    let handing = std::thread::spawn({
        let out = out.clone();
        move || {
            until_some("the command to run", || (runs(&out) == 1).then_some(()));
            daemon.replace(None)
        }
    });
    match api.try_ask(new_pane(writing("op-1", "lost", &out))) {
        Tried::Unknown(Some(ended)) => assert_eq!(ended.reason(), api::ended::Reason::Replaced),
        other => panic!("the request's fate is unknown, not {other:?}"),
    }
    let answer = handing.join().unwrap();
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
}

/// While a handoff is under way a repeat is still answered from its record, and only an operation
/// not seen before is turned away, leaving nothing behind for the daemon taking over.
#[test]
fn during_a_handoff_a_repeat_gets_its_result_and_only_a_new_operation_is_unavailable() {
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-hold")]);
    let out = daemon.root().join("out");
    let mut api = daemon.api();
    let first = api.ask(new_pane(writing("op-1", "once", &out)));
    assert_eq!(first.outcome(), api::Outcome::Done, "{:?}", first.answer);

    let replacing = daemon.start_replacing(None);
    daemon.paused();
    let again = api.ask(new_pane(writing("op-1", "once", &out)));
    assert_eq!(again.outcome(), api::Outcome::AlreadySo, "{:?}", again.answer);
    assert_eq!(made(&again), made(&first));
    let turned_away = api.ask(new_pane(writing("op-2", "later", &out)));
    assert_eq!(
        (turned_away.outcome(), turned_away.refusal()),
        (api::Outcome::Refused, api::Refusal::Unavailable),
        "{:?}",
        turned_away.answer
    );
    daemon.resume();
    let answer = daemon.finish_replacing(replacing);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    let later = daemon.api().ask(new_pane(writing("op-2", "later", &out)));
    assert_eq!(later.outcome(), api::Outcome::Done, "{:?}", later.answer);
}

/// A repeat of an operation still starting when a handoff begins is unknown, never refused: its
/// command has run.
#[test]
fn during_a_handoff_a_repeat_of_an_operation_under_way_is_unknown() {
    let mut daemon = daemon_with(&[
        ("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-hold"),
        ("MUSTER_DAEMON_API_STARTED_DELAY", "3000"),
    ]);
    let out = daemon.root().join("out");
    let starting = daemon.api();
    let mut writer = starting.writer();
    let asked = writing("op-1", "lost", &out);
    let request =
        api::Request { correlation: "c1".to_string(), request: Some(new_pane(asked.clone())) };
    muster_api::connection::send(&mut writer, &request).unwrap();
    until_some("the command to run", || (runs(&out) == 1).then_some(()));
    let mut api = daemon.api();

    let replacing = daemon.start_replacing(None);
    daemon.paused();
    let again = api.ask(new_pane(asked.clone()));
    assert_eq!(again.outcome(), api::Outcome::Unknown, "{:?}", again.answer);
    daemon.resume();
    let answer = daemon.finish_replacing(replacing);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    let after = answered(daemon.api().try_ask(new_pane(asked)));
    assert_eq!(after.outcome(), api::Outcome::Unknown, "{:?}", after.answer);
    assert_eq!(runs(&out), 1);
}
