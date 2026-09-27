//! Rearranging panes and tabs, and what each change announces.

use crate::support::*;
use muster_harness::Input;
use proto::input_event::{self, Input as Event};
use proto::{Side, pane_request, tab_request};

/// A tab `t1` holding `[a|b]`, on a subscribed connection.
fn two_panes() -> (Daemon, Control) {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    make(&mut control, create("a", in_new_tab("t1")));
    make(&mut control, create("b", beside("a", Side::Right)));
    (daemon, control)
}

fn zoom(name: &str, zoomed: bool) -> proto::request::Service {
    pane(pane_request::Request::Zoom(pane_request::Zoom { pane: name.to_string(), zoomed }))
}

fn swap(one: &str, with: &str) -> proto::request::Service {
    pane(pane_request::Request::Swap(pane_request::Swap {
        pane: one.to_string(),
        with: with.to_string(),
    }))
}

fn moved(name: &str, placement: proto::Placement) -> proto::request::Service {
    pane(pane_request::Request::Move(pane_request::Move {
        pane: name.to_string(),
        placement: Some(placement),
    }))
}

fn resize(name: &str, direction: Side, fraction: Option<f32>) -> proto::request::Service {
    pane(pane_request::Request::Resize(pane_request::Resize {
        pane: name.to_string(),
        direction: direction.into(),
        fraction,
    }))
}

fn rename_tab(tab: &str, text: Option<&str>, generation: u64) -> proto::request::Service {
    self::tab(tab_request::Request::Rename(tab_request::Rename {
        tab: tab.to_string(),
        label: Some(proto::Label { text: text.map(str::to_string), generation }),
    }))
}

#[test]
fn closing_a_tab_closes_every_pane_in_it() {
    let (_daemon, mut control) = two_panes();
    let close = tab(tab_request::Request::Close(tab_request::Close { tab: "t1".to_string() }));
    let asked = expect(&mut control, close.clone(), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_closed:t1", "pane_closed:a", "pane_closed:b"]);
    assert!(snapshot(&mut control).panes.is_empty());
    expect(&mut control, close, proto::Outcome::NotThere);
}

#[test]
fn zoom_is_a_state_and_asking_for_the_one_it_is_in_says_so() {
    let (_daemon, mut control) = two_panes();
    let asked = expect(&mut control, zoom("b", true), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_changed:t1"]);
    let tab = &snapshot(&mut control).tabs[0];
    assert_eq!(tab.zoomed.as_deref(), Some("b"));
    expect(&mut control, zoom("b", true), proto::Outcome::AlreadySo);
    expect(&mut control, zoom("a", false), proto::Outcome::AlreadySo);
    expect(&mut control, zoom("b", false), proto::Outcome::Done);
    expect(&mut control, zoom("missing", true), proto::Outcome::NotThere);
}

#[test]
fn a_zoomed_pane_leaving_its_tab_unzooms_it() {
    let (_daemon, mut control) = two_panes();
    expect(&mut control, zoom("b", true), proto::Outcome::Done);
    expect(&mut control, close_request("b"), proto::Outcome::Done);
    assert_eq!(snapshot(&mut control).tabs[0].zoomed, None);
}

#[test]
fn swapping_exchanges_two_panes_in_one_tab() {
    let (_daemon, mut control) = two_panes();
    let asked = expect(&mut control, swap("a", "b"), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_changed:t1"]);
    assert_eq!(tab_shape(&mut control, "t1"), "[b|a 0.50]");
    expect(&mut control, swap("a", "a"), proto::Outcome::AlreadySo);
    make(&mut control, create("c", in_new_tab("t2")));
    expect(&mut control, swap("a", "c"), proto::Outcome::Refused);
}

#[test]
fn moving_a_pane_beside_another_in_its_own_tab_announces_the_tab_once() {
    let (_daemon, mut control) = two_panes();
    let asked = expect(&mut control, moved("a", beside("b", Side::Down)), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_changed:t1"]);
    assert_eq!(tab_shape(&mut control, "t1"), "[b/a 0.50]");
    expect(&mut control, moved("a", beside("a", Side::Down)), proto::Outcome::Refused);
}

#[test]
fn moving_a_pane_out_announces_the_tab_it_left_and_then_where_it_went() {
    let (_daemon, mut control) = two_panes();
    let asked = expect(&mut control, moved("b", in_new_tab("t2")), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_changed:t1", "tab_opened:t2"]);
    assert_eq!(tab_shape(&mut control, "t1"), "a");
    assert_eq!(tab_shape(&mut control, "t2"), "b");

    // Moving the last pane out of a tab closes it.
    let asked = expect(&mut control, moved("a", beside("b", Side::Left)), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_closed:t1", "tab_changed:t2"]);
    assert_eq!(tab_shape(&mut control, "t2"), "[a|b 0.50]");
    expect(&mut control, moved("a", in_new_tab("t2")), proto::Outcome::Refused);
}

#[test]
fn a_ratio_is_set_by_the_path_to_its_split() {
    let (_daemon, mut control) = two_panes();
    let set = |ratio: f32| {
        tab(tab_request::Request::SetSplitRatio(tab_request::SetSplitRatio {
            tab: "t1".to_string(),
            path: Vec::new(),
            ratio,
        }))
    };
    let asked = expect(&mut control, set(0.25), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_changed:t1"]);
    assert_eq!(tab_shape(&mut control, "t1"), "[a|b 0.25]");
    expect(&mut control, set(0.25), proto::Outcome::AlreadySo);
    expect(&mut control, set(1.5), proto::Outcome::Refused);
    let deeper = tab(tab_request::Request::SetSplitRatio(tab_request::SetSplitRatio {
        tab: "t1".to_string(),
        path: vec![proto::Branch::First.into()],
        ratio: 0.5,
    }));
    expect(&mut control, deeper, proto::Outcome::Refused);
}

#[test]
fn resizing_moves_the_nearest_divider_and_says_when_it_can_go_no_further() {
    let (_daemon, mut control) = two_panes();
    expect(&mut control, resize("b", Side::Left, Some(0.2)), proto::Outcome::Done);
    assert_eq!(tab_shape(&mut control, "t1"), "[a|b 0.30]");
    expect(&mut control, resize("a", Side::Left, Some(0.5)), proto::Outcome::Done);
    assert_eq!(tab_shape(&mut control, "t1"), "[a|b 0.10]");
    expect(&mut control, resize("a", Side::Left, None), proto::Outcome::AlreadySo);
    expect(&mut control, resize("a", Side::Up, None), proto::Outcome::Refused);
}

#[test]
fn renaming_a_pane_announces_its_record() {
    let (_daemon, mut control) = two_panes();
    let rename = |label: Option<&str>| {
        pane(pane_request::Request::Rename(pane_request::Rename {
            pane: "a".to_string(),
            label: label.map(str::to_string),
        }))
    };
    let asked = expect(&mut control, rename(Some("🔥 spike")), proto::Outcome::Done);
    let Some(proto::event::Event::PaneChanged(changed)) = &asked.events[0].event else {
        panic!("a rename announced {:?}", names(&asked.events));
    };
    assert_eq!(changed.pane.as_ref().unwrap().label.as_deref(), Some("🔥 spike"));
    expect(&mut control, rename(Some("🔥 spike")), proto::Outcome::AlreadySo);
    expect(&mut control, rename(None), proto::Outcome::Done);
    assert_eq!(snapshot(&mut control).panes[0].label, None);
}

/// A name somebody gave a pane and the title its program set are two things, and setting either
/// leaves the other as it was.
#[test]
fn a_panes_name_and_its_programs_title_are_kept_apart() {
    let (daemon, mut control) = two_panes();
    let mut input = Input::connect(daemon.socket_path());
    let mut set_title = |title: &str| {
        let text = format!("printf '\\033]2;{title}\\007'");
        input.send("a", Event::Send(input_event::Send { text, enter: true }));
    };
    let record = |control: &mut Control| {
        snapshot(control).panes.into_iter().find(|record| record.pane == "a").unwrap()
    };

    set_title("first working build");
    until_some("a's title", || (record(&mut control).title == "first working build").then_some(()));
    let rename = pane_request::Rename { pane: "a".to_string(), label: Some("🤖 A".to_string()) };
    let asked =
        expect(&mut control, pane(pane_request::Request::Rename(rename)), proto::Outcome::Done);
    let Some(proto::event::Event::PaneChanged(changed)) = &asked.events[0].event else {
        panic!("a rename announced {:?}", names(&asked.events));
    };
    let changed = changed.pane.as_ref().unwrap();
    assert_eq!(
        (changed.label.as_deref(), changed.title.as_str()),
        (Some("🤖 A"), "first working build")
    );

    set_title("second");
    until_some("a's new title beside its name", || {
        let now = record(&mut control);
        (now.title == "second" && now.label.as_deref() == Some("🤖 A")).then_some(())
    });
}

#[test]
fn a_tab_name_older_than_the_one_recorded_is_refused() {
    let (_daemon, mut control) = two_panes();
    let asked = expect(&mut control, rename_tab("t1", Some("work"), 2), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_changed:t1"]);
    expect(&mut control, rename_tab("t1", Some("work"), 2), proto::Outcome::AlreadySo);
    let stale = expect(&mut control, rename_tab("t1", Some("old"), 1), proto::Outcome::Refused);
    assert!(stale.answer.reason.contains("generation 2"), "{}", stale.answer.reason);
    expect(&mut control, rename_tab("t1", None, 3), proto::Outcome::Done);
    let label = snapshot(&mut control).tabs[0].label.clone().unwrap();
    assert_eq!(label, proto::Label { text: None, generation: 3 });
    expect(&mut control, rename_tab("missing", Some("x"), 9), proto::Outcome::NotThere);
}
