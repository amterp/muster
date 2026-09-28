//! Requests spelled so a test reads as what it asks for, and the waits that go with them.

use proto::request::Service;
use proto::{pane_request, placement, session_request, tab_request};

use crate::control::{Asked, Control};
use crate::until::{PATIENCE, until_some};
use muster_daemon_proto as proto;

pub fn session(request: session_request::Request) -> Service {
    Service::Session(proto::SessionRequest { request: Some(request) })
}

pub fn tab(request: tab_request::Request) -> Service {
    Service::Tab(proto::TabRequest { request: Some(request) })
}

pub fn pane(request: pane_request::Request) -> Service {
    Service::Pane(proto::PaneRequest { request: Some(request) })
}

pub fn snapshot_request() -> Service {
    session(session_request::Request::Snapshot(session_request::Snapshot {}))
}

pub fn subscribe_request() -> Service {
    session(session_request::Request::Subscribe(session_request::Subscribe::default()))
}

/// A subscription as a window on the daemon's own machine asks for it, attending: what wakes
/// the human.
pub fn attending_request() -> Service {
    session(session_request::Request::Subscribe(session_request::Subscribe { attends: true }))
}

pub fn in_new_tab(tab: &str) -> proto::Placement {
    proto::Placement {
        r#where: Some(placement::Where::NewTab(placement::NewTab {
            tab: tab.to_string(),
            label: None,
        })),
    }
}

pub fn beside(pane: &str, side: proto::Side) -> proto::Placement {
    proto::Placement {
        r#where: Some(placement::Where::Beside(placement::Beside {
            pane: pane.to_string(),
            side: side.into(),
            ratio: None,
        })),
    }
}

/// A create with nothing but a name and a place.
pub fn create(name: &str, placement: proto::Placement) -> pane_request::Create {
    pane_request::Create {
        pane: name.to_string(),
        placement: Some(placement),
        ..pane_request::Create::default()
    }
}

pub fn create_request(create: pane_request::Create) -> Service {
    pane(pane_request::Request::Create(create))
}

pub fn close_request(name: &str) -> Service {
    pane(pane_request::Request::Close(pane_request::Close { pane: name.to_string() }))
}

/// Asks and insists on an outcome, naming the reason when it differs.
#[track_caller]
pub fn expect(control: &mut Control, service: Service, outcome: proto::Outcome) -> Asked {
    let asked = control.ask(service);
    assert_eq!(
        asked.outcome(),
        outcome,
        "the daemon answered {:?}: {}",
        asked.outcome(),
        asked.answer.reason
    );
    asked
}

/// Makes a pane and insists it was made.
#[track_caller]
pub fn make(control: &mut Control, create: pane_request::Create) -> Asked {
    expect(control, create_request(create), proto::Outcome::Done)
}

pub fn snapshot(control: &mut Control) -> proto::Snapshot {
    match expect(control, snapshot_request(), proto::Outcome::Done).answer.detail {
        Some(proto::answer::Detail::Snapshot(snapshot)) => snapshot,
        other => panic!("a snapshot request answered with {other:?}"),
    }
}

/// A tab's tree as text: `[a|b .50]` for columns, `[a/b .50]` for rows.
pub fn shape(node: &proto::Node) -> String {
    match node.node.as_ref().expect("a node holds something") {
        proto::node::Node::Pane(name) => name.clone(),
        proto::node::Node::Split(split) => {
            let divider = if split.axis() == proto::Axis::Columns { "|" } else { "/" };
            format!(
                "[{}{divider}{} {:.2}]",
                shape(split.first.as_ref().expect("a split has a first child")),
                shape(split.second.as_ref().expect("a split has a second child")),
                split.ratio
            )
        }
    }
}

/// The shape of the tab named `tab`, from a fresh snapshot.
pub fn tab_shape(control: &mut Control, tab: &str) -> String {
    let snapshot = snapshot(control);
    let found = snapshot.tabs.iter().find(|candidate| candidate.tab == tab);
    shape(found.unwrap_or_else(|| panic!("no tab {tab} in {snapshot:?}")).root.as_ref().unwrap())
}

/// An event as `kind:name`, which is what most ordering assertions care about.
pub fn named(event: &proto::Event) -> String {
    use proto::event::Event as E;
    match event.event.as_ref().expect("an event holds something") {
        E::PaneOpened(opened) => format!("pane_opened:{}", opened.pane.as_ref().unwrap().pane),
        E::PaneChanged(changed) => format!("pane_changed:{}", changed.pane.as_ref().unwrap().pane),
        E::PaneClosed(closed) => format!("pane_closed:{}", closed.pane),
        E::TabOpened(opened) => format!("tab_opened:{}", opened.tab.as_ref().unwrap().tab),
        E::TabChanged(changed) => format!("tab_changed:{}", changed.tab.as_ref().unwrap().tab),
        E::TabClosed(closed) => format!("tab_closed:{}", closed.tab),
        E::SettingsChanged(_) => "settings_changed".to_string(),
        E::PaneEffect(effect) => format!("pane_effect:{}", effect.pane),
        E::PasteHeld(held) => format!("paste_held:{}", held.pane),
        E::Restored(restored) => format!("restored:{}", restored.lost_tabs.join(",")),
        E::Replaced(replaced) => format!("replaced:{}", replaced.daemon_version),
        E::HumanNotice(notice) => format!("human_notice:{}", notice.group),
    }
}

pub fn names(events: &[proto::Event]) -> Vec<String> {
    events.iter().map(named).collect()
}

pub fn read_request(name: &str, first_row: u64, rows: u32) -> Service {
    pane(pane_request::Request::Read(pane_request::Read {
        pane: name.to_string(),
        first_row,
        rows,
        last: 0,
    }))
}

/// A page of a pane's text, insisting it was read.
#[track_caller]
pub fn read_text(control: &mut Control, name: &str, first_row: u64, rows: u32) -> proto::PaneText {
    match expect(control, read_request(name, first_row, rows), proto::Outcome::Done).answer.detail {
        Some(proto::answer::Detail::Text(text)) => text,
        other => panic!("a read answered with {other:?}"),
    }
}

/// Every row of a pane's text, once it contains `needle`.
pub fn until_text(control: &mut Control, name: &str, needle: &str) -> String {
    until_some(&format!("pane {name} to show {needle:?}"), || {
        Some(read_text(control, name, 0, 0).text).filter(|text| text.contains(needle))
    })
}

/// Events from a subscribed connection until `enough` says the ones gathered suffice.
pub fn events_until(
    control: &mut Control,
    what: &str,
    mut enough: impl FnMut(&[proto::Event]) -> bool,
) -> Vec<proto::Event> {
    let deadline = std::time::Instant::now() + PATIENCE;
    let mut events = Vec::new();
    while !enough(&events) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(!left.is_zero(), "{what}: not within the suite's patience; heard {events:?}");
        if let Some(proto::control_message::Message::Event(event)) = control.next_message(left) {
            events.push(event);
        }
    }
    events
}
