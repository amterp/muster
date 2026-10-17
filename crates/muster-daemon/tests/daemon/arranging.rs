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

/// A hidden tab is an ordinary tab that says so: a pane moves into one and out again as into any
/// other, and the flag is the tab's from the moment it is made.
#[test]
fn a_pane_moves_into_a_hidden_tab_and_out_again() {
    let (_daemon, mut control) = two_panes();
    let hidden = |control: &mut Control, tab: &str| {
        snapshot(control).tabs.into_iter().find(|held| held.tab == tab).map(|held| held.hidden)
    };
    let asked = expect(&mut control, moved("b", in_new_hidden_tab("t2")), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_changed:t1", "tab_opened:t2"]);
    let Some(proto::event::Event::TabOpened(opened)) = &asked.events[1].event else {
        panic!("a move announced {:?}", names(&asked.events));
    };
    assert!(opened.tab.as_ref().unwrap().hidden, "the tab is announced hidden");
    assert_eq!(hidden(&mut control, "t2"), Some(true));
    assert_eq!(hidden(&mut control, "t1"), Some(false));

    // A pane made beside a hidden one is in its hidden tab.
    make(&mut control, create("c", beside("b", Side::Down)));
    assert_eq!(tab_shape(&mut control, "t2"), "[b/c 0.50]");

    let asked = expect(&mut control, moved("b", in_new_tab("t3")), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_changed:t2", "tab_opened:t3"]);
    assert_eq!(hidden(&mut control, "t3"), Some(false));
    expect(&mut control, moved("c", in_new_tab("t4")), proto::Outcome::Done);
    assert_eq!(hidden(&mut control, "t2"), None, "its last pane leaving closes a hidden tab");
}

#[test]
fn a_pane_can_start_in_a_hidden_tab_and_quiet() {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    let asked = expect(
        &mut control,
        create_request(pane_request::Create {
            quiet: true,
            ..create("a", in_new_hidden_tab("t1"))
        }),
        proto::Outcome::Done,
    );
    assert_eq!(names(&asked.events), ["pane_opened:a", "tab_opened:t1"]);
    let held = snapshot(&mut control);
    assert!(held.tabs[0].hidden);
    assert!(held.panes[0].quiet);
}

/// A maker's hidden helpers share one tab, named when the first of them makes it, and each one
/// joins it as a column as wide as the rest. Another maker's helpers get a tab of their own.
#[test]
fn a_makers_helpers_share_one_hidden_tab() {
    let (_daemon, mut control) = two_panes();
    let asked = make(&mut control, helper("h1", "a", among_helpers("t2", "a's helpers")));
    assert_eq!(names(&asked.events), ["pane_opened:h1", "tab_opened:t2"]);
    make(&mut control, helper("h2", "a", among_helpers("t3", "a's helpers")));
    make(&mut control, helper("h3", "a", among_helpers("t4", "a's helpers")));
    assert_eq!(tab_shape(&mut control, "t2"), "[[h1|h2 0.50]|h3 0.67]");
    make(&mut control, helper("g1", "b", among_helpers("t5", "b's helpers")));

    let held = snapshot(&mut control);
    let tabs: Vec<(&str, bool, Option<&str>)> = held
        .tabs
        .iter()
        .map(|tab| {
            let label = tab.label.as_ref().and_then(|label| label.text.as_deref());
            (tab.tab.as_str(), tab.hidden, label)
        })
        .collect();
    assert_eq!(
        tabs,
        [("t1", false, None), ("t2", true, Some("a's helpers")), ("t5", true, Some("b's helpers"))],
        "one hidden tab per maker, and no tab for a name that went unused"
    );
    let maker = |pane: &str| {
        held.panes.iter().find(|held| held.pane == pane).and_then(|held| held.maker.clone())
    };
    assert_eq!(maker("h2").as_deref(), Some("a"));
    assert_eq!(maker("g1").as_deref(), Some("b"));
    assert_eq!(maker("a"), None, "a pane nobody made as a helper has no maker");

    // A helper split beside its maker is still a helper, and its tab is the maker's.
    make(&mut control, helper("h4", "a", beside("a", Side::Down)));
    assert_eq!(maker_of(&mut control, "h4").as_deref(), Some("a"));
    assert_eq!(tab_shape(&mut control, "t1"), "[[a/h4 0.50]|b 0.50]");
    // Helpers of helpers, and a maker on another machine, are recorded as asked.
    make(&mut control, helper("h5", "p2c6txyt94", among_helpers("t6", "far's helpers")));
    assert_eq!(maker_of(&mut control, "h5").as_deref(), Some("p2c6txyt94"));
}

/// A hidden tab is brought back whole, its panes arranged as they were, and hidden again: what
/// showing and hiding a maker's helpers asks of the daemon. A name sent with it is taken in the
/// same change, unless the tab was renamed since.
#[test]
fn a_hidden_tab_is_shown_and_hidden_again_whole() {
    let (_daemon, mut control) = two_panes();
    make(&mut control, helper("h1", "a", among_helpers("t2", "a's helpers")));
    make(&mut control, helper("h2", "a", among_helpers("t3", "a's helpers")));
    let set = |hidden: bool, text: Option<&str>, generation: u64| {
        tab(tab_request::Request::SetHidden(tab_request::SetHidden {
            tab: "t2".to_string(),
            hidden,
            label: text.map(|text| proto::Label { text: Some(text.to_string()), generation }),
        }))
    };
    let held = |control: &mut Control| {
        let held = snapshot(control);
        let tab = held.tabs.into_iter().find(|tab| tab.tab == "t2").expect("t2 is still held");
        (tab.hidden, tab.label.and_then(|label| label.text))
    };

    let asked = expect(&mut control, set(false, Some("A's helpers"), 1), proto::Outcome::Done);
    assert_eq!(names(&asked.events), ["tab_changed:t2"]);
    assert_eq!(held(&mut control), (false, Some("A's helpers".to_string())));
    assert_eq!(tab_shape(&mut control, "t2"), "[h1|h2 0.50]", "arranged as it was");
    expect(&mut control, set(false, None, 0), proto::Outcome::AlreadySo);

    expect(&mut control, set(true, None, 0), proto::Outcome::Done);
    assert_eq!(held(&mut control), (true, Some("A's helpers".to_string())));

    let rename = tab_request::Rename {
        tab: "t2".to_string(),
        label: Some(proto::Label { text: Some("mine".to_string()), generation: 3 }),
    };
    expect(&mut control, tab(tab_request::Request::Rename(rename)), proto::Outcome::Done);
    expect(&mut control, set(false, Some("B's helpers"), 1), proto::Outcome::Done);
    assert_eq!(held(&mut control), (false, Some("mine".to_string())), "a person's name stays");
}

/// Where a helper goes is decided as it is placed, after its shell has started, so helpers made
/// at the same moment on different connections still end up in one tab.
#[test]
fn helpers_made_at_once_still_share_one_tab() {
    let (daemon, mut control) = two_panes();
    std::thread::scope(|scope| {
        for (name, tab) in [("h1", "t2"), ("h2", "t3"), ("h3", "t4")] {
            let daemon = &daemon;
            scope.spawn(move || {
                let mut control = daemon.connect();
                make(&mut control, helper(name, "a", among_helpers(tab, "a's helpers")));
            });
        }
    });
    let hidden: Vec<String> = snapshot(&mut control)
        .tabs
        .into_iter()
        .filter(|tab| tab.hidden)
        .map(|tab| tab.tab)
        .collect();
    assert_eq!(hidden.len(), 1, "one tab of helpers: {hidden:?}");
}

fn maker_of(control: &mut Control, pane: &str) -> Option<String> {
    snapshot(control).panes.into_iter().find(|held| held.pane == pane).and_then(|held| held.maker)
}

#[test]
fn a_helper_needs_a_maker_other_than_itself_and_only_a_create_places_one() {
    let (_daemon, mut control) = two_panes();
    let refused = |control: &mut Control, create: pane_request::Create, why: &str| {
        let asked = expect(control, create_request(create), proto::Outcome::Refused);
        assert!(asked.answer.reason.contains(why), "{}", asked.answer.reason);
    };
    refused(&mut control, create("c", among_helpers("t2", "x")), "needs a maker");
    refused(&mut control, helper("c", "c", among_helpers("t2", "x")), "its own helper");
    refused(&mut control, helper("c", "a b", among_helpers("t2", "x")), "maker");
    refused(&mut control, helper("c", "a", among_helpers("t1", "x")), "already on this daemon");
    let asked = expect(&mut control, moved("b", among_helpers("t2", "x")), proto::Outcome::Refused);
    assert!(asked.answer.reason.contains("only a pane being made"), "{}", asked.answer.reason);
    assert_eq!(tab_shape(&mut control, "t1"), "[a|b 0.50]", "nothing moved");
}

fn quiet(name: &str, quiet: bool) -> proto::request::Service {
    pane(pane_request::Request::Quiet(pane_request::Quiet { pane: name.to_string(), quiet }))
}

#[test]
fn making_a_pane_quiet_announces_its_record() {
    let (_daemon, mut control) = two_panes();
    let asked = expect(&mut control, quiet("a", true), proto::Outcome::Done);
    let Some(proto::event::Event::PaneChanged(changed)) = &asked.events[0].event else {
        panic!("quieting announced {:?}", names(&asked.events));
    };
    assert!(changed.pane.as_ref().unwrap().quiet);
    expect(&mut control, quiet("a", true), proto::Outcome::AlreadySo);
    expect(&mut control, quiet("b", false), proto::Outcome::AlreadySo);
    expect(&mut control, quiet("a", false), proto::Outcome::Done);
    assert!(snapshot(&mut control).panes.iter().all(|record| !record.quiet));
    expect(&mut control, quiet("z", true), proto::Outcome::NotThere);
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
        input.send("a", Event::Send(input_event::Send { text, enter: true, ..Default::default() }));
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
