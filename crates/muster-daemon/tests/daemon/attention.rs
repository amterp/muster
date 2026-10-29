//! What waits on the human, kept by the daemon whether or not a window is open, and the human's
//! presence as a window reports it (MIP-10, section 4). A control connection stands in for the
//! app throughout: one subscribing as attending is a window on this machine, and one subscribing
//! plainly is a client that reads and tells nobody anything.

use crate::support::*;
use proto::msg_request::{self, Request as Asked};
use proto::request::Service;
use proto::session_request;

const DONE: proto::Outcome = proto::Outcome::Done;

fn msg(caller: &msg_request::Caller, asked: Asked) -> Service {
    Service::Msg(proto::MsgRequest { caller: Some(caller.clone()), request: Some(asked) })
}

fn named(name: &str) -> msg_request::Caller {
    msg_request::Caller { as_name: Some(name.to_string()), ..msg_request::Caller::default() }
}

/// A caller carrying no agent identity, which is the human (MIP-4, section 3).
fn the_human() -> msg_request::Caller {
    msg_request::Caller::default()
}

fn join(control: &mut Control, caller: &msg_request::Caller, name: &str, group: &str) {
    let asked = Asked::Join(msg_request::Join {
        name: Some(name.to_string()),
        group: Some(group.to_string()),
        pull: false,
    });
    expect(control, msg(caller, asked), DONE);
}

/// `a` writes to the human in group `g`, which both are in.
fn a_message_for_the_human(control: &mut Control) {
    join(control, &the_human(), "@human", "g");
    join(control, &named("a"), "a", "g");
    let post = msg_request::Post {
        to: vec!["@human".to_string()],
        body: "need input".to_string(),
        ..Default::default()
    };
    expect(control, msg(&named("a"), Asked::Post(post)), DONE);
}

fn subscribed(control: &mut Control, service: Service) -> proto::Snapshot {
    match expect(control, service, DONE).answer.detail {
        Some(proto::answer::Detail::Snapshot(snapshot)) => snapshot,
        other => panic!("a subscription answered with {other:?}"),
    }
}

/// What waits for the human in each group, as `(group, count)` for every group where something
/// does.
fn waiting(snapshot: &proto::Snapshot) -> Vec<(String, u64)> {
    snapshot
        .human
        .iter()
        .filter(|notice| notice.count > 0)
        .map(|notice| (notice.group.clone(), notice.count))
        .collect()
}

fn asking(snapshot: &proto::Snapshot) -> Vec<(String, proto::Ask, bool)> {
    snapshot
        .asking
        .iter()
        .map(|asking| (asking.pane.clone(), asking.ask(), asking.unheard_helper))
        .collect()
}

fn presence(snapshot: &proto::Snapshot) -> proto::Presence {
    snapshot.presence.expect("a snapshot says the human's presence")
}

/// Reads events until one `pick` takes, failing if none does within the suite's patience each.
fn until_event<T>(control: &mut Control, pick: impl Fn(proto::event::Event) -> Option<T>) -> T {
    loop {
        if let Some(found) = control.next_event().event.and_then(&pick) {
            return found;
        }
    }
}

fn next_presence(control: &mut Control) -> proto::Presence {
    until_event(control, |event| match event {
        proto::event::Event::Presence(presence) => Some(presence),
        _ => None,
    })
}

fn next_asking(control: &mut Control, pane: &str) -> proto::Asking {
    until_event(control, |event| match event {
        proto::event::Event::Asking(asking) if asking.pane == pane => Some(asking),
        _ => None,
    })
}

fn report(in_front: bool, last_input_ms: Option<u64>, lasts_ms: u64) -> Service {
    session(session_request::Request::ReportPresence(proto::ReportPresence {
        in_front,
        last_input_ms,
        lasts_ms,
    }))
}

/// A quiet helper of `maker`, among its helpers.
fn quiet_helper(name: &str, maker: &str) -> proto::pane_request::Create {
    proto::pane_request::Create {
        quiet: true,
        ..helper(name, maker, among_helpers("t2", "helpers"))
    }
}

/// With no window open, a message for the human and a blocked agent both wait, a client that
/// reads them changes nothing, and one connecting afresh finds them still waiting.
#[test]
fn with_no_window_what_waits_on_the_human_waits_for_any_client_to_find() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    daemon.set_agent_state("p1", proto::AgentState::Blocked);
    a_message_for_the_human(&mut control);

    let mut client = daemon.connect();
    let first = subscribed(&mut client, subscribe_request());
    assert_eq!(waiting(&first), [("g".to_string(), 1)]);
    assert_eq!(asking(&first), [("p1".to_string(), proto::Ask::Blocked, false)]);
    let nobody = proto::Presence::default();
    assert_eq!(presence(&first), nobody, "no window, so no presence");
    // Reading what waits reads nothing for the human.
    assert_eq!(waiting(&snapshot(&mut client)), [("g".to_string(), 1)]);
    drop(client);

    let mut again = daemon.connect();
    let second = subscribed(&mut again, subscribe_request());
    assert_eq!(waiting(&second), [("g".to_string(), 1)]);
    assert_eq!(asking(&second), asking(&first));
    assert_eq!(presence(&second), nobody);
}

/// The human reading the group at the desk clears its message, and the agent going back to work
/// stops it asking: each an event to every subscriber.
#[test]
fn reading_at_the_desk_and_an_agent_going_back_to_work_clear_what_waits() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let mut client = daemon.connect();
    assert_eq!(asking(&subscribed(&mut client, subscribe_request())), []);

    daemon.set_agent_state("p1", proto::AgentState::Blocked);
    assert_eq!(next_asking(&mut client, "p1").ask(), proto::Ask::Blocked);
    a_message_for_the_human(&mut control);

    let mut window = daemon.connect();
    subscribed(&mut window, attending_request());
    let read = Asked::Read(msg_request::Read { group: Some("g".to_string()) });
    expect(&mut window, msg(&the_human(), read), DONE);
    let cleared = until_event(&mut client, |event| match event {
        proto::event::Event::HumanNotice(notice) if notice.count == 0 => Some(notice.group),
        _ => None,
    });
    assert_eq!(cleared, "g");

    daemon.set_agent_state("p1", proto::AgentState::Working);
    assert_eq!(next_asking(&mut client, "p1").ask(), proto::Ask::Unspecified);
    let now = snapshot(&mut client);
    assert_eq!((waiting(&now), asking(&now)), (Vec::new(), Vec::new()));
}

/// A window's report stands while it is renewed and reads as unknown once it runs out, with the
/// window still connected, as a hung app's would; a window that goes takes its presence with it
/// at once, as a crashed or quit app's does.
#[test]
fn presence_runs_out_without_reports_and_goes_with_its_window() {
    let daemon = daemon();
    let mut client = daemon.connect();
    assert_eq!(presence(&subscribed(&mut client, subscribe_request())), proto::Presence::default());

    let mut window = daemon.connect();
    subscribed(&mut window, attending_request());
    let attended = next_presence(&mut client);
    assert_eq!((attended.attended, attended.known), (true, false), "attending, nothing reported");

    expect(&mut window, report(true, Some(1_000), 1_500), DONE);
    let present = next_presence(&mut client);
    assert_eq!((present.known, present.in_front, present.last_input_ms), (true, true, Some(1_000)));
    assert_eq!(presence(&snapshot(&mut client)), present);

    let ran_out = next_presence(&mut client);
    assert_eq!((ran_out.attended, ran_out.known), (true, false), "{ran_out:?}");

    expect(&mut window, report(false, Some(2_000), 60_000), DONE);
    assert!(next_presence(&mut client).known);
    drop(window);
    let gone = next_presence(&mut client);
    assert_eq!(gone, proto::Presence::default(), "a window that went left presence behind");
}

/// Presence is the attending window's to report: a client that only reads cannot claim it.
#[test]
fn a_connection_that_does_not_attend_cannot_report_presence() {
    let daemon = daemon();
    let mut client = daemon.connect();
    subscribed(&mut client, subscribe_request());
    let refused = expect(&mut client, report(true, None, 0), proto::Outcome::Refused);
    assert!(refused.answer.reason.contains("attending"), "{}", refused.answer.reason);
    assert!(!presence(&snapshot(&mut client)).known);
}

/// What the app showed the human reaches every subscriber, and changes nothing else.
#[test]
fn what_the_app_showed_reaches_every_subscriber() {
    let daemon = daemon();
    let mut client = daemon.connect();
    subscribed(&mut client, subscribe_request());
    let mut window = daemon.connect();
    let shown = proto::Shown {
        about: Some(proto::shown::About::Group("g".to_string())),
        shown_as: proto::ShownAs::Banner.into(),
        why: "message".to_string(),
    };
    expect(&mut window, session(session_request::Request::Shown(shown.clone())), DONE);
    let heard = until_event(&mut client, |event| match event {
        proto::event::Event::Shown(shown) => Some(shown),
        _ => None,
    });
    assert_eq!(heard, shown);

    let empty = proto::Shown::default();
    expect(&mut window, session(session_request::Request::Shown(empty)), proto::Outcome::Refused);
}

/// A quiet helper asks for nobody while its maker holds an agent to hear from it, asks as an
/// unheard helper once the maker's agent leaves, and asks plainly once the maker's pane closes,
/// which makes it no longer quiet (MIP-7, sections 4 and 6).
#[test]
fn a_quiet_helper_asks_for_the_human_only_while_its_maker_cannot_hear_it() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    make(&mut control, quiet_helper("p2", "p1"));
    daemon.run_agent("p2");
    daemon.set_agent_state("p2", proto::AgentState::Blocked);
    let mut client = daemon.connect();
    assert_eq!(asking(&subscribed(&mut client, subscribe_request())), [], "its maker hears it");

    daemon.type_into("p1", "quit");
    let unheard = next_asking(&mut client, "p2");
    assert_eq!((unheard.ask(), unheard.unheard_helper), (proto::Ask::Blocked, true));

    expect(&mut control, close_request("p1"), DONE);
    let orphaned = next_asking(&mut client, "p2");
    assert_eq!((orphaned.ask(), orphaned.unheard_helper), (proto::Ask::Blocked, false));
}

/// A quiet pane nobody made asks for nobody, whatever its agent does.
#[test]
fn a_quiet_pane_that_is_no_helper_never_asks() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    let quiet = proto::pane_request::Create { quiet: true, ..create("p1", in_new_tab("t1")) };
    make(&mut control, quiet);
    daemon.run_agent("p1");
    // What a pane asks is worked out as its record changes, so it is settled by the time the
    // record says blocked.
    daemon.set_agent_state("p1", proto::AgentState::Blocked);
    assert_eq!(asking(&snapshot(&mut control)), []);
}

/// A quiet helper whose maker is on another machine asks as unheard: this daemon cannot see
/// whether anything there holds an agent to hear it.
#[test]
fn a_quiet_helper_whose_maker_is_elsewhere_asks_as_unheard() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, quiet_helper("p2", "far1"));
    daemon.run_agent("p2");
    daemon.set_agent_state("p2", proto::AgentState::Blocked);
    assert_eq!(asking(&snapshot(&mut control)), [("p2".to_string(), proto::Ask::Blocked, true)]);
}

/// A daemon that takes over another's panes finds a pane already asking, with nothing having
/// changed in it since.
#[test]
fn a_pane_asking_before_a_handoff_asks_after_it() {
    let mut daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    daemon.set_agent_state("p1", proto::AgentState::Blocked);
    drop(control);

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), DONE, "{}", answer.reason);
    let mut client = daemon.connect();
    let after = subscribed(&mut client, subscribe_request());
    assert_eq!(asking(&after), [("p1".to_string(), proto::Ask::Blocked, false)]);
}
