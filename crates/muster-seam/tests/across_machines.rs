//! Which machine a request lands on, in a window showing two of them.
//!
//! A pane's name is Muster's own and unique across every attached machine, which is what lets
//! `muster pane send --pane p1w3r07bsd` work without the caller knowing where that pane lives.
//! Every verb that takes such a name has to honour it, and the failure when one does not is
//! silent: the request goes to whichever machine the keyboard happens to be on, and comes back
//! blaming the pane for not being there. `muster tab new` did exactly that (kan a_2Hwef7lQT).
//!
//! So the sweep below is per verb rather than per behaviour. They all take the same kind of
//! ref, and a verb added later that resolves it its own way would break here and nowhere else.
//!
//! Two real daemons, both local. What separates the machines in the code under test is the id
//! and the socket, and this stages both; ssh would add a transport and no new question. The
//! ids are `laptop` and `devenv` rather than `local` and something, so that no assertion here
//! can pass because a name happened to be the one Muster falls back to.

use std::sync::Mutex;

use herdr_harness::{Daemon, until};
use muster::proto::{
    ArrangePane, ClosePane, CreateTab, Event, FocusPane, OpenWindow, PaneText, ReadPane,
    RenamePane, Request, Response, RosterChanged, SendToPane, SplitPane, Startup, ZoomPane, event,
    request, response,
};
use prost::Message;
use serde_json::{Value, json};

/// A window showing a laptop and a devenv, each holding one named pane.
struct TwoMachines {
    laptop: Daemon,
    devenv: Daemon,
}

/// The name each machine's only pane is given, so a test can find it again.
///
/// Distinct across the two machines on purpose. Muster mints a name per pane and nothing here
/// can predict it, so the given name is the only thing this test and the core both know - and
/// two panes called the same thing would make "which machine is this row on" unanswerable from
/// the list.
const ON_LAPTOP: &str = "on-laptop";
const ON_DEVENV: &str = "on-devenv";

/// Text typed into the laptop's pane, and read back two ways: off the daemon's own screen and
/// through `muster pane read`.
const SENT: &str = "muster-across-machines";

fn a_window_showing_two_machines() -> TwoMachines {
    let machines = two_machines(Devenv::HoldingAPane);
    until(
        "the devenv to reach the list with its pane",
        || pane_on("devenv").is_some(),
        || format!("the list holds {:?}", rows()),
    );
    machines
}

/// The same window with a devenv that has never held anything, which is what a machine named
/// in the config and never used looks like: attached, listed, and holding no workspace at all.
fn a_window_and_an_untouched_devenv() -> TwoMachines {
    two_machines(Devenv::HoldingNothing)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Devenv {
    HoldingAPane,
    HoldingNothing,
}

fn two_machines(devenv: Devenv) -> TwoMachines {
    let laptop = Daemon::start();
    let second = Daemon::start();
    a_workspace_holding_one_named_pane(&laptop, ON_LAPTOP);
    if devenv == Devenv::HoldingAPane {
        a_workspace_holding_one_named_pane(&second, ON_DEVENV);
    }

    let config = laptop.muster_config_naming("laptop", &[("devenv", &second)]);
    watch();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    // Only the laptop, because whether the devenv ends up with a pane is what half the tests
    // here are asking about.
    until(
        "the laptop to reach the list with its pane",
        || pane_on("laptop").is_some(),
        || format!("the list holds {:?}", rows()),
    );
    TwoMachines { laptop, devenv: second }
}

/// One workspace holding one pane, named before Muster has heard of any of it.
///
/// Named through the daemon rather than through Muster, and before startup, because herdr
/// announces a rename to nobody: the bootstrap snapshot is the only thing that carries one
/// (`observations/herdr-0.8.0.md` section 16).
fn a_workspace_holding_one_named_pane(daemon: &Daemon, given: &str) {
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": given, "focus": true }));
    let pane = only_pane(daemon);
    daemon.call("pane.rename", &json!({ "pane_id": pane, "label": given }));
}

/// The card's own check, one verb at a time.
///
/// The keyboard sits on the devenv throughout and every request names the laptop's pane. A
/// verb that took the machine from the keyboard would send the laptop's pane to the devenv,
/// which either refuses by name or - worse for a verb the daemon does not validate - acts on
/// something else entirely. Both are caught by asking each daemon what it holds afterwards.
///
/// One test rather than one per verb, because the fixture is two real daemons and the answer
/// is the same question asked eight ways. The order is not arbitrary: a split has to come
/// before a move has two panes to swap, and a tab comes last because making one brings it on
/// screen - which moves the laptop's region off the tab every other check is aimed at.
#[test]
fn every_verb_that_names_a_pane_acts_on_that_panes_machine() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();
    let (laptop, devenv) = (&laptop, &devenv);

    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let on_devenv = pane_on("devenv").expect("the fixture waited for it");
    let laptop_pane = backend_id(laptop, ON_LAPTOP);

    // The keyboard, put on the devenv and left there. This is the whole staging: everything
    // below names a pane on the other machine, so anything that reads the keyboard is wrong.
    put_the_keyboard_back(&on_devenv);

    a_focus_moves_the_keyboard_to_the_named_machine(&on_laptop, &on_devenv);
    a_split_lands_on_the_named_machine(laptop, devenv, &on_laptop);
    a_zoom_fills_the_named_machines_region(&on_laptop);
    text_reaches_the_named_machines_pane(laptop, &on_laptop, &laptop_pane);
    a_read_answers_with_the_named_machines_pane(&on_laptop);
    a_rename_reaches_the_named_machines_pane(laptop, &on_laptop, &laptop_pane);
    a_move_rearranges_the_named_machines_tab(&on_laptop);
    a_close_takes_a_pane_off_the_named_machine(laptop, devenv, &on_laptop);
    a_tab_is_made_on_the_named_machine(laptop, devenv, &on_laptop);
}

/// `muster focus`. First, because a focus that reached the wrong machine would leave every
/// check after it staged against a keyboard that is not where this test thinks it is.
fn a_focus_moves_the_keyboard_to_the_named_machine(on_laptop: &str, on_devenv: &str) {
    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        daemon_id: String::new(),
        pane_id: on_laptop.to_string(),
    })));
    until(
        "the keyboard to reach the laptop's pane, on the laptop",
        || keyboard() == Some(("laptop".to_string(), on_laptop.to_string())),
        || format!("the keyboard is on {:?}", keyboard()),
    );
    put_the_keyboard_back(on_devenv);
}

/// `muster pane new`, counted on both machines.
fn a_split_lands_on_the_named_machine(laptop: &Daemon, devenv: &Daemon, on_laptop: &str) {
    let (before, elsewhere) = (panes(laptop).len(), panes(devenv).len());
    assert_ok(&answer(request::Payload::SplitPane(SplitPane {
        pane_id: on_laptop.to_string(),
        side: "right".to_string(),
        ..SplitPane::default()
    })));
    until(
        "the laptop to hold one more pane than it did",
        || panes(laptop).len() == before + 1,
        || format!("the laptop holds {:?}", panes(laptop)),
    );
    assert_eq!(
        panes(devenv).len(),
        elsewhere,
        "a split asked of the laptop appeared on the devenv, which is where the keyboard was"
    );
}

/// `muster zoom`, and the one check here that asks the *window* about the machine rather than
/// only the daemon: a zoom needs a region holding the pane, and the devenv's region does not.
///
/// Sent twice, because herdr's `pane.zoom` defaults to toggling - so this leaves the tab the
/// way it found it for the checks that follow.
fn a_zoom_fills_the_named_machines_region(on_laptop: &str) {
    let zoom = || {
        assert_ok(&answer(request::Payload::ZoomPane(ZoomPane {
            pane_id: on_laptop.to_string(),
            ..ZoomPane::default()
        })));
    };
    zoom();
    until(
        "the laptop's region to be filled by the pane that was named",
        || zoomed_on("laptop").as_deref() == Some(on_laptop),
        || format!("the laptop's region is showing {:?}", zoomed_on("laptop")),
    );
    zoom();
    until(
        "the laptop's region to go back to showing its tree",
        || zoomed_on("laptop").is_none(),
        || format!("the laptop's region is still zoomed on {:?}", zoomed_on("laptop")),
    );
}

/// `muster pane send`, read back off the laptop's own screen. The daemon renders every pane
/// whether or not anything is attached to it, so this is a usable oracle with no bridge.
fn text_reaches_the_named_machines_pane(laptop: &Daemon, on_laptop: &str, laptop_pane: &str) {
    assert_ok(&answer(request::Payload::SendToPane(SendToPane {
        pane_id: on_laptop.to_string(),
        text: SENT.to_string(),
        ..SendToPane::default()
    })));
    until(
        "the text to arrive on the laptop's pane",
        || screen(laptop, laptop_pane).contains(SENT),
        || format!("the laptop's pane shows {:?}", screen(laptop, laptop_pane)),
    );
}

/// `muster pane read`, which has to answer with that machine's pane rather than refuse.
fn a_read_answers_with_the_named_machines_pane(on_laptop: &str) {
    let read = read_pane(on_laptop);
    assert!(
        read.text.contains(SENT),
        "reading the laptop's pane answered without the text just typed into it: {:?}",
        read.text
    );
}

/// `muster pane rename`. The oracle is the laptop's own label, so a rename that reached the
/// devenv leaves this waiting rather than passing on a name nobody can see.
fn a_rename_reaches_the_named_machines_pane(laptop: &Daemon, on_laptop: &str, laptop_pane: &str) {
    assert_ok(&answer(request::Payload::RenamePane(RenamePane {
        pane_id: on_laptop.to_string(),
        name: "renamed".to_string(),
        ..RenamePane::default()
    })));
    until(
        "the laptop's pane to carry the name it was given",
        || label(laptop, laptop_pane) == "renamed",
        || format!("the laptop's pane is called {:?}", label(laptop, laptop_pane)),
    );
}

/// `muster pane move`, which names two panes and no machine at all.
///
/// Read back off the list rather than off the daemon's snapshot, because a swap changes how a
/// tab is laid out and not which panes it holds - and the list is where Muster puts a tab's
/// panes in the order its tree lays them out.
fn a_move_rearranges_the_named_machines_tab(on_laptop: &str) {
    let beside = beside_it(on_laptop);
    let before = panes_on("laptop");
    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: on_laptop.to_string(),
        onto_pane_id: beside,
        ..ArrangePane::default()
    })));
    until(
        "the laptop's two panes to have traded places",
        || panes_on("laptop") != before,
        || format!("the laptop's panes read {:?}, and read {before:?}", panes_on("laptop")),
    );
}

/// `muster pane close`, on the pane the split made rather than on the one everything above
/// asserted against.
fn a_close_takes_a_pane_off_the_named_machine(laptop: &Daemon, devenv: &Daemon, on_laptop: &str) {
    let (before, elsewhere) = (panes(laptop).len(), panes(devenv).len());
    assert_ok(&answer(request::Payload::ClosePane(ClosePane {
        pane_id: beside_it(on_laptop),
        ..ClosePane::default()
    })));
    until(
        "the laptop to hold one fewer pane than it did",
        || panes(laptop).len() == before - 1,
        || format!("the laptop holds {:?}", panes(laptop)),
    );
    assert_eq!(
        panes(devenv).len(),
        elsewhere,
        "closing one of the laptop's panes took one off the devenv, which is where the \
         keyboard was"
    );
}

/// `muster tab new`, the verb the card is about.
///
/// herdr's `tab.create` takes a workspace and ignores keys it does not know, so a tab asked of
/// the wrong daemon does not merely fail - this is the one verb here where the wrong machine
/// could have made a tab somewhere nobody asked for. Counted on both sides for that reason.
fn a_tab_is_made_on_the_named_machine(laptop: &Daemon, devenv: &Daemon, on_laptop: &str) {
    let (before, elsewhere) = (tabs(laptop), tabs(devenv));
    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        pane_id: on_laptop.to_string(),
        ..CreateTab::default()
    })));
    until(
        "the laptop to hold one more tab than it did",
        || tabs(laptop) == before + 1,
        || format!("the laptop holds {} tabs and the devenv {}", tabs(laptop), tabs(devenv)),
    );
    assert_eq!(
        tabs(devenv),
        elsewhere,
        "a tab asked of the laptop appeared on the devenv, which is where the keyboard was"
    );
}

/// The laptop's other pane, by Muster's name for it.
fn beside_it(pane: &str) -> String {
    panes_on("laptop")
        .into_iter()
        .find(|held| held != pane)
        .expect("the split gave the laptop a second pane")
}

/// The half that must not change: a request naming no pane still means the keyboard's machine.
///
/// This is what ⌘T sends, and it is the case `resolve_daemon` was written for. A fix that
/// took the daemon from the named pane and forgot this would move ⌘T onto whichever machine
/// happened to be first in the config file.
#[test]
fn a_tab_naming_no_pane_lands_where_the_keyboard_is() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();
    let (laptop, devenv) = (&laptop, &devenv);

    let on_devenv = pane_on("devenv").expect("the fixture waited for it");
    put_the_keyboard_back(&on_devenv);

    let (laptop_tabs, devenv_tabs) = (tabs(laptop), tabs(devenv));
    // What ⌘T sends: no daemon, no pane, and the keyboard following.
    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        take_focus: true,
        ..CreateTab::default()
    })));
    until(
        "the devenv to hold one more tab than it did",
        || tabs(devenv) == devenv_tabs + 1,
        || format!("the laptop holds {} tabs and the devenv {}", tabs(laptop), tabs(devenv)),
    );
    assert_eq!(
        tabs(laptop),
        laptop_tabs,
        "⌘T on the devenv made a tab on the laptop, so a request that names no pane is no \
         longer about the machine the keyboard is on"
    );
}

/// A machine attached for the first time, and the pane it has no other way to get.
///
/// The window's own half of kan a_2HpkpfIfq. Before this, such a machine attached, appeared in
/// the agent list saying `no tabs, so this daemon is holding nothing`, and stayed that way:
/// every route to a new pane goes through an existing one, so there was nothing in Muster that
/// could give it a first. `herdr workspace create` against the forwarded socket was the way
/// through, which is not a way through for anybody who did not already know that.
///
/// Nothing is asked for here. The window asks on its own, the way it already did for a window
/// showing nothing at all - which is what makes this a fix for the machine rather than for the
/// window it happened to be alone in.
#[test]
fn a_machine_that_has_never_held_a_pane_is_given_one() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_and_an_untouched_devenv();

    until(
        "the untouched machine to be given a pane of its own",
        || panes(&devenv).len() == 1,
        || format!("the devenv holds {:?}", panes(&devenv)),
    );
    until(
        "the window to show the pane it gave that machine",
        || panes_on("devenv").len() == 1,
        || format!("the list holds {:?}", rows()),
    );
    assert_eq!(
        panes(&devenv).len(),
        1,
        "the machine was given more than one workspace, so the rule that asks is asking again \
         while its own answer is still in flight"
    );
    // The laptop is untouched by any of it: a rule that filled every machine it could reach
    // would be a rule that opens panes nobody asked for on the machine you are working on.
    assert_eq!(panes(&laptop).len(), 1, "the laptop was given a pane it did not need");
}

/// `muster pane new --daemon <id>`, which is the same reach from a script.
///
/// The machine is named and the keyboard is somewhere else, so the pane that gets split has to
/// be the one *that machine's* region has the keyboard on. Taking the window's own keyboard
/// pane and sending it to the named machine is the defect kan a_2Hwef7lQT was about, and
/// `--daemon` is the one caller that produces that combination on purpose.
#[test]
fn a_split_asked_for_by_machine_lands_on_that_machine() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();

    let on_devenv = pane_on("devenv").expect("the fixture waited for it");
    put_the_keyboard_back(&on_devenv);

    let (before, elsewhere) = (panes(&laptop).len(), panes(&devenv).len());
    assert_ok(&answer(request::Payload::SplitPane(SplitPane {
        daemon_id: "laptop".to_string(),
        side: "right".to_string(),
        ..SplitPane::default()
    })));
    until(
        "the laptop to hold one more pane than it did",
        || panes(&laptop).len() == before + 1,
        || format!("the laptop holds {:?}", panes(&laptop)),
    );
    assert_eq!(
        panes(&devenv).len(),
        elsewhere,
        "naming the laptop split a pane on the devenv, which is where the keyboard was"
    );
}

/// A machine name that reaches no machine, refused by name.
///
/// `--daemon` is the first field a person types a machine into, so a typo in one is a thing
/// that will happen. Without this it reaches the backend and comes back as "which is a bug in
/// the core rather than a state to recover from", which sends somebody looking for a bug in
/// Muster instead of at what they typed.
#[test]
fn a_machine_this_window_is_not_following_is_refused_by_name() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop: _laptop, devenv: _devenv } = a_window_showing_two_machines();

    let reason = refusal(request::Payload::SplitPane(SplitPane {
        daemon_id: "typo".to_string(),
        side: "right".to_string(),
        ..SplitPane::default()
    }));
    for expected in ["typo", "laptop", "devenv"] {
        assert!(
            reason.contains(expected),
            "a refusal for a machine that is not there should name it and the ones that are, \
             and did not mention {expected}: {reason}"
        );
    }
}

fn put_the_keyboard_back(on_devenv: &str) {
    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        daemon_id: String::new(),
        pane_id: on_devenv.to_string(),
    })));
    until(
        "the keyboard to be back on the devenv",
        || keyboard() == Some(("devenv".to_string(), on_devenv.to_string())),
        || format!("the keyboard is on {:?}", keyboard()),
    );
}

// --- what the core has published -------------------------------------------------------

/// The last list of everything the daemons hold, which is what a sidebar row comes from.
static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);

/// The last view, for the questions only the window can answer - the keyboard, and a zoom.
static VIEW: Mutex<Option<muster::proto::ViewChanged>> = Mutex::new(None);

fn watch() {
    *ROSTER.lock().expect("a panicking reader poisoned the roster") = None;
    *VIEW.lock().expect("a panicking reader poisoned the view") = None;
    muster::ffi::muster_set_event_callback(Some(note));
}

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    match Event::decode(bytes) {
        Ok(Event { payload: Some(event::Payload::RosterChanged(roster)) }) => {
            *ROSTER.lock().expect("a panicking reader poisoned the roster") = Some(roster);
        }
        Ok(Event { payload: Some(event::Payload::ViewChanged(view)) }) => {
            *VIEW.lock().expect("a panicking reader poisoned the view") = Some(view);
        }
        _ => {}
    }
}

/// Every listed pane, as the machine holding it, the name somebody gave it, and Muster's name.
fn rows() -> Vec<(String, String, String)> {
    ROSTER
        .lock()
        .expect("a panicking reader poisoned the roster")
        .as_ref()
        .into_iter()
        .flat_map(|roster| roster.daemons.iter())
        .flat_map(|daemon| {
            daemon.tabs.iter().flat_map(move |tab| {
                tab.panes.iter().map(move |pane| {
                    (daemon.daemon_id.clone(), pane.given_name.clone(), pane.pane_id.clone())
                })
            })
        })
        .collect()
}

/// What Muster calls the one pane this fixture gave `daemon`, once the core has listed it.
fn pane_on(daemon: &str) -> Option<String> {
    let given = if daemon == "laptop" { ON_LAPTOP } else { ON_DEVENV };
    rows()
        .into_iter()
        .find_map(|(held, name, pane)| (held == daemon && name == given).then_some(pane))
}

/// Every pane the list holds for one machine, by Muster's name for it.
fn panes_on(daemon: &str) -> Vec<String> {
    rows().into_iter().filter(|(held, ..)| held == daemon).map(|(_, _, pane)| pane).collect()
}

/// Which machine the window's keyboard is on, and which of its panes.
fn keyboard() -> Option<(String, String)> {
    let view = VIEW.lock().expect("a panicking reader poisoned the view").clone()?;
    let region = view.regions.into_iter().find(|region| region.region_id == view.focused_region)?;
    (!region.pane_id.is_empty()).then_some((region.daemon_id, region.pane_id))
}

/// The pane filling one machine's region, when one is.
fn zoomed_on(daemon: &str) -> Option<String> {
    let region = VIEW
        .lock()
        .expect("a panicking reader poisoned the view")
        .clone()?
        .regions
        .into_iter()
        .find(|region| region.daemon_id == daemon)?;
    region.zoomed.then_some(region.pane_id)
}

fn read_pane(pane: &str) -> PaneText {
    match answer(request::Payload::ReadPane(ReadPane {
        pane_id: pane.to_string(),
        ..ReadPane::default()
    }))
    .payload
    {
        Some(response::Payload::PaneText(text)) => text,
        other => panic!("expected the text of {pane}, got {other:?}"),
    }
}

// --- what each daemon says it holds ----------------------------------------------------

fn snapshot(daemon: &Daemon) -> Value {
    daemon.call("session.snapshot", &json!({}))["snapshot"].clone()
}

fn tabs(daemon: &Daemon) -> usize {
    snapshot(daemon)["tabs"].as_array().map_or(0, Vec::len)
}

/// Every pane this daemon holds, by its own id for it, in the order it lists them.
fn panes(daemon: &Daemon) -> Vec<String> {
    snapshot(daemon)["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| pane.get("pane_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn only_pane(daemon: &Daemon) -> String {
    let panes = panes(daemon);
    assert_eq!(panes.len(), 1, "a fresh workspace should hold exactly one pane: {panes:?}");
    panes[0].clone()
}

/// This daemon's own id for the pane it labels `given`.
fn backend_id(daemon: &Daemon, given: &str) -> String {
    snapshot(daemon)["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|pane| pane.get("label").and_then(Value::as_str) == Some(given))
        .and_then(|pane| pane.get("pane_id").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("this daemon holds no pane labelled {given}"))
        .to_string()
}

fn label(daemon: &Daemon, pane: &str) -> String {
    snapshot(daemon)["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|held| held.get("pane_id").and_then(Value::as_str) == Some(pane))
        .and_then(|held| held.get("label").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// What a pane is showing, asked of the daemon that renders it.
///
/// Unwrapped, because a pane in a split is about two dozen columns wide and a line that wraps
/// comes back with a newline through the middle of it.
fn screen(daemon: &Daemon, pane: &str) -> String {
    daemon.call("pane.read", &json!({ "pane_id": pane, "source": "recent_unwrapped" }))["read"]
        ["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// --- driving the seam ------------------------------------------------------------------

fn answer(payload: request::Payload) -> Response {
    let request = Request { payload: Some(payload) };
    Response::decode(muster::dispatch(&request.encode_to_vec()).as_slice())
        .expect("the core answers every request with a decodable response")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => panic!("the core refused: {}", failure.reason),
        None => panic!("the core answered with no payload"),
        Some(_) => {}
    }
}

/// The reason a request was refused, or a panic saying it was not.
fn refusal(payload: request::Payload) -> String {
    match answer(payload).payload {
        Some(response::Payload::Failure(failure)) => failure.reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}
