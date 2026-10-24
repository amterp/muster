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

use muster::proto::{
    AdjustFontSize, ArrangePane, ClosePane, CreateTab, EqualizePanes, Event, FocusPane, OpenWindow,
    PaneText, ReadPane, ReadWindow, RenamePane, RenameTab, Request, ResizePane, Response,
    RosterChanged, SendToPane, SetSplitRatio, SplitPane, Startup, ZoomPane, event, request,
    response,
};
use muster_daemon_proto as daemon_proto;
use muster_harness::requests::{
    among_helpers, close_request, create, expect, helper, in_new_tab, make, read_text,
    set_zoomed_tree_request, shape,
};
use muster_harness::{Daemon, until};
use prost::Message;

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

/// What each machine's only pane and its tab are called. Muster uses a daemon's names as its
/// own, so these are also the names every request here addresses - distinct across the two
/// machines, because a pane's name is meant to be unique across every machine a window shows.
const LAPTOP_PANE: &str = "p1laptop00";
const LAPTOP_TAB: &str = "t1laptop00";
const DEVENV_PANE: &str = "p1devenv00";
const DEVENV_TAB: &str = "t1devenv00";

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
/// in the config and never used looks like: attached, listed, and holding no tab at all.
fn a_window_and_an_untouched_devenv() -> TwoMachines {
    two_machines(Devenv::HoldingNothing)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Devenv {
    HoldingAPane,
    HoldingNothing,
}

fn two_machines(devenv: Devenv) -> TwoMachines {
    two_machines_after(devenv, |_, _| {})
}

/// [`two_machines`], with `prepare` given the laptop's daemon and the devenv's before the window
/// opens onto them.
fn two_machines_after(devenv: Devenv, prepare: impl FnOnce(&Daemon, &Daemon)) -> TwoMachines {
    let laptop = Daemon::start_built();
    let second = Daemon::start_built();
    a_tab_holding_one_named_pane(&laptop, LAPTOP_TAB, LAPTOP_PANE, ON_LAPTOP);
    if devenv == Devenv::HoldingAPane {
        a_tab_holding_one_named_pane(&second, DEVENV_TAB, DEVENV_PANE, ON_DEVENV);
    }
    prepare(&laptop, &second);

    let config = laptop.muster_config_naming("laptop", &[("devenv", &second)]);
    watch();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

    // Only the laptop, because whether the devenv ends up with a pane is what half the tests
    // here are asking about.
    until(
        "the laptop to reach the list with its pane",
        || pane_on("laptop").is_some(),
        || format!("the list holds {:?}", rows()),
    );
    TwoMachines { laptop, devenv: second }
}

/// A machine added to the config is attached when the file is saved, and its panes come into the
/// window. One taken out stays attached until Muster is relaunched, because detaching it would
/// take running agents' tabs out of the window, and a warning says so (kan a_2JtRvPUH5).
#[test]
fn a_machine_added_to_the_config_is_attached_and_one_taken_out_waits_for_a_relaunch() {
    let _turn = muster::testing::fresh_session();
    let laptop = Daemon::start_built();
    let second = Daemon::start_built();
    a_tab_holding_one_named_pane(&laptop, LAPTOP_TAB, LAPTOP_PANE, ON_LAPTOP);
    a_tab_holding_one_named_pane(&second, DEVENV_TAB, DEVENV_PANE, ON_DEVENV);
    let both =
        std::fs::read_to_string(laptop.muster_config_naming("laptop", &[("devenv", &second)]))
            .expect("the config was written");
    let config = laptop.muster_config_naming("laptop", &[]);
    let devenv_only = format!(
        "[[daemon]]{}",
        both.split("[[daemon]]").nth(2).expect("the second block is the devenv's")
    );
    watch();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the laptop's pane to be listed",
        || pane_on("laptop").is_some(),
        || format!("the list holds {:?}", rows()),
    );

    std::fs::write(&config, &both).expect("the config can be written");
    assert_ok(&answer(request::Payload::ReloadConfig(muster::proto::ReloadConfig {})));
    until(
        "the devenv added to the config to be attached, with its pane",
        || pane_on("devenv").is_some(),
        || format!("the list holds {:?}", rows()),
    );

    std::fs::write(&config, &devenv_only).expect("the config can be written");
    assert_ok(&answer(request::Payload::ReloadConfig(muster::proto::ReloadConfig {})));
    let waiting = PROBLEMS
        .lock()
        .expect("a panicking reader poisoned the problems")
        .clone()
        .and_then(|changed| changed.problems.into_iter().find(|p| p.key == "config.daemons"));
    let waiting = waiting.expect("taking a machine out of the config says it waits for a relaunch");
    assert_eq!(waiting.severity, "warning");
    assert!(waiting.detail.contains("laptop"), "it names the machine: {}", waiting.detail);
    assert!(pane_on("laptop").is_some(), "the laptop's pane left the window: {:?}", rows());
}

/// A machine added with a mistake in its block never attaches, so it has no panes to keep:
/// correcting the block and saving again attaches it from the corrected endpoint, rather than
/// leaving it waiting for a relaunch that the old attach, still retrying the wrong one, needs.
#[test]
fn a_machine_that_never_attached_is_attached_once_its_block_is_corrected() {
    let _turn = muster::testing::fresh_session();
    let laptop = Daemon::start_built();
    let second = Daemon::start_built();
    a_tab_holding_one_named_pane(&laptop, LAPTOP_TAB, LAPTOP_PANE, ON_LAPTOP);
    a_tab_holding_one_named_pane(&second, DEVENV_TAB, DEVENV_PANE, ON_DEVENV);
    let both =
        std::fs::read_to_string(laptop.muster_config_naming("laptop", &[("devenv", &second)]))
            .expect("the config was written");
    let socket = second.socket_path().to_string_lossy().into_owned();
    assert!(both.contains(&socket), "the config names the devenv's socket: {both}");
    let mistyped = both.replace(&socket, &format!("{socket}.mistyped"));
    let config = laptop.muster_config_naming("laptop", &[]);
    watch();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the laptop's pane to be listed",
        || pane_on("laptop").is_some(),
        || format!("the list holds {:?}", rows()),
    );

    std::fs::write(&config, &mistyped).expect("the config can be written");
    assert_ok(&answer(request::Payload::ReloadConfig(muster::proto::ReloadConfig {})));
    std::fs::write(&config, &both).expect("the config can be written");
    assert_ok(&answer(request::Payload::ReloadConfig(muster::proto::ReloadConfig {})));
    until(
        "the corrected devenv to be attached, with its pane",
        || pane_on("devenv").is_some(),
        || {
            let problems = PROBLEMS.lock().expect("a panicking reader poisoned the problems");
            format!("the list holds {:?}, and the problems {:?}", rows(), problems.clone())
        },
    );
    let problems = PROBLEMS
        .lock()
        .expect("a panicking reader poisoned the problems")
        .clone()
        .map(|changed| changed.problems)
        .unwrap_or_default();
    assert!(
        problems.iter().all(|problem| problem.key != "config.daemons"),
        "a corrected block was said to wait for a relaunch: {problems:?}"
    );
}

/// One tab holding one pane, made on the daemon before Muster has heard of any of it, which is
/// what a machine somebody has been working on looks like.
fn a_tab_holding_one_named_pane(daemon: &Daemon, tab: &str, pane: &str, given: &str) {
    make(
        &mut daemon.connect(),
        daemon_proto::pane_request::Create {
            label: Some(given.to_string()),
            ..create(pane, in_new_tab(tab))
        },
    );
}

/// A quiet helper on `daemon`, `pane`, whose maker is `maker`.
fn a_quiet_helper(daemon: &Daemon, pane: &str, maker: &str) {
    let quiet = daemon_proto::pane_request::Create {
        quiet: true,
        ..helper(pane, maker, among_helpers("t2devenv00", "its maker's helpers"))
    };
    make(&mut daemon.connect(), quiet);
}

/// Whether `daemon` holds `pane` as a helper whose maker has closed, no longer quiet.
fn freed(daemon: &Daemon, pane: &str) -> bool {
    snapshot(daemon)
        .panes
        .into_iter()
        .any(|held| held.pane == pane && held.maker_closed && !held.quiet)
}

/// A helper's daemon cannot see a maker on another machine close, and the window can: it tells
/// the helper's daemon, which stops the helper being quiet (MIP-7, section 6).
#[test]
fn a_maker_closing_on_one_machine_frees_its_helper_on_the_other() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = two_machines_after(Devenv::HoldingAPane, |_, devenv| {
        a_quiet_helper(devenv, "p2devenv00", LAPTOP_PANE);
    });
    until(
        "the devenv to reach the list with its pane",
        || pane_on("devenv").is_some(),
        || format!("the list holds {:?}", rows()),
    );
    assert!(!freed(&devenv, "p2devenv00"), "freed while its maker was open");

    expect(&mut laptop.connect(), close_request(LAPTOP_PANE), daemon_proto::Outcome::Done);
    until(
        "the devenv to hear its helper's maker closed",
        || freed(&devenv, "p2devenv00"),
        || format!("the devenv holds {:?}", snapshot(&devenv).panes),
    );
}

/// A maker that closed while no window was open is told of when one opens: a helper whose maker
/// neither machine holds, once both have answered (MIP-7, section 6).
#[test]
fn a_window_opening_frees_a_helper_whose_maker_neither_machine_holds() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { devenv, .. } = two_machines_after(Devenv::HoldingAPane, |_, devenv| {
        a_quiet_helper(devenv, "p2devenv00", "p9gone0000");
    });
    until(
        "the devenv to be told its helper's maker has closed",
        || freed(&devenv, "p2devenv00"),
        || format!("the devenv holds {:?}", snapshot(&devenv).panes),
    );
}

/// The card's own check, one verb at a time.
///
/// The keyboard sits on the devenv throughout and every request names the laptop's pane. A
/// verb that took the machine from the keyboard would send the laptop's pane to the devenv,
/// which either refuses by name or - worse for a verb the daemon does not validate - acts on
/// something else entirely. Both are caught by asking each daemon what it holds afterwards.
///
/// One test rather than one per verb, because the fixture is two real daemons and the answer
/// is the same question asked nine ways. The order is not arbitrary: a split has to come
/// before a move has two panes to swap, and a tab comes last because making one brings it on
/// screen - which moves the laptop's region off the tab every other check is aimed at.
#[test]
fn every_verb_that_names_a_pane_acts_on_that_panes_machine() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();
    let (laptop, devenv) = (&laptop, &devenv);

    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let on_devenv = pane_on("devenv").expect("the fixture waited for it");

    // The keyboard, put on the devenv and left there. This is the whole staging: everything
    // below names a pane on the other machine, so anything that reads the keyboard is wrong.
    put_the_keyboard_back(&on_devenv);

    a_focus_moves_the_keyboard_to_the_named_machine(&on_laptop, &on_devenv);
    a_split_lands_on_the_named_machine(laptop, devenv, &on_laptop);
    a_zoom_fills_the_named_machines_region(&on_laptop, &on_devenv);
    text_reaches_the_named_machines_pane(laptop, &on_laptop);
    a_read_answers_with_the_named_machines_pane(&on_laptop);
    a_rename_reaches_the_named_machines_pane(laptop, &on_laptop);
    a_move_rearranges_the_named_machines_tab(&on_laptop);
    a_close_takes_a_pane_off_the_named_machine(laptop, devenv, &on_laptop);
    a_tab_is_made_on_the_named_machine(laptop, devenv, &on_laptop);
}

/// A pane is reached by its name wherever its id is, on whichever machine holds it. Two machines
/// can share a name, and a request by that name alone is refused - naming each pane by its
/// callsign and machine - until it says which.
#[test]
fn a_pane_is_reached_by_its_name_and_a_shared_name_by_its_machine() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();
    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let on_devenv = pane_on("devenv").expect("the fixture waited for it");
    put_the_keyboard_back(&on_devenv);

    let rename = |pane: &str| {
        assert_ok(&answer(request::Payload::RenamePane(RenamePane {
            pane_id: pane.to_string(),
            name: "twin".to_string(),
            ..RenamePane::default()
        })));
    };
    rename(ON_LAPTOP);
    rename(ON_DEVENV);
    until(
        "both panes to be called twin, each on its own machine",
        || {
            label(&laptop, &on_laptop) == "twin"
                && label(&devenv, &on_devenv) == "twin"
                && rows().iter().filter(|(_, name, _)| name == "twin").count() == 2
        },
        || format!("the list holds {:?}", rows()),
    );

    let refused = refusal(request::Payload::FocusPane(FocusPane {
        daemon_id: String::new(),
        pane_id: "twin".to_string(),
    }));
    assert!(
        refused.contains(&format!("@laptop ({on_laptop})"))
            && refused.contains(&format!("@devenv ({on_devenv})")),
        "the refusal names each by its callsign and machine: {refused}"
    );

    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        daemon_id: String::new(),
        pane_id: "twin@laptop".to_string(),
    })));
    until(
        "the keyboard to reach the laptop's twin",
        || keyboard() == Some(("laptop".to_string(), on_laptop.clone())),
        || format!("the keyboard is on {:?}", keyboard()),
    );
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
/// only the daemon.
///
/// Asked for with the keyboard on the devenv, like everything else here, and looked at by going
/// to the laptop's pane afterwards. A window shows one tab at a time, so the laptop's half of
/// the arrangement is not drawn while the devenv's tab is - which is exactly why the request
/// must not need it to be: zooming a pane in a tab you are not looking at is ordinary, and its
/// effect is there when you arrive.
///
/// Sent twice, because a zoom request toggles - the window reads which state the pane is in and
/// asks for the other - so this leaves the tab the way it found it for the checks that follow.
fn a_zoom_fills_the_named_machines_region(on_laptop: &str, on_devenv: &str) {
    let zoom = || {
        assert_ok(&answer(request::Payload::ZoomPane(ZoomPane {
            pane_id: on_laptop.to_string(),
            ..ZoomPane::default()
        })));
    };
    let go_to = |pane: &str| {
        assert_ok(&answer(request::Payload::FocusPane(FocusPane {
            daemon_id: String::new(),
            pane_id: pane.to_string(),
        })));
    };
    zoom();
    go_to(on_laptop);
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
    put_the_keyboard_back(on_devenv);
}

/// `muster pane send`, read back off the laptop's own screen. The daemon keeps every pane's
/// screen whether or not anything is attached to it, so this is a usable oracle with no bridge.
fn text_reaches_the_named_machines_pane(laptop: &Daemon, on_laptop: &str) {
    assert_ok(&answer(request::Payload::SendToPane(SendToPane {
        pane_id: on_laptop.to_string(),
        text: SENT.to_string(),
        ..SendToPane::default()
    })));
    until(
        "the text to arrive on the laptop's pane",
        || screen(laptop, on_laptop).contains(SENT),
        || format!("the laptop's pane shows {:?}", screen(laptop, on_laptop)),
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
fn a_rename_reaches_the_named_machines_pane(laptop: &Daemon, on_laptop: &str) {
    assert_ok(&answer(request::Payload::RenamePane(RenamePane {
        pane_id: on_laptop.to_string(),
        name: "renamed".to_string(),
        ..RenamePane::default()
    })));
    until(
        "the laptop's pane to carry the name it was given",
        || label(laptop, on_laptop) == "renamed",
        || format!("the laptop's pane is called {:?}", label(laptop, on_laptop)),
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
/// A daemon makes whatever tab it is asked for, so a tab asked of the wrong daemon does not
/// merely fail - this is the one verb here where the wrong machine would make a tab somewhere
/// nobody asked for. Counted on both sides for that reason.
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

/// A machine attached for the first time: left alone, and reachable when asked.
///
/// Both halves of kan a_2HpkpfIfq and kan a_2I6h18OU6, which pull in opposite directions and
/// settle here. Muster no longer fills a machine it has not been asked to fill - the rule that
/// gave every machine a column of its own is gone, and with it the devenv shell that appeared
/// over ssh a moment after you closed the last one. What replaces it is a way in: the machine
/// is listed whether or not it holds anything, and one request gives it a pane.
///
/// The request is the one the machine's row in the agent list sends, and the one
/// `muster tab new --daemon <id>` sends from a script.
#[test]
fn a_machine_that_has_never_held_a_pane_is_left_alone_until_it_is_asked_for() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_and_an_untouched_devenv();

    until(
        "the laptop to reach the list, so the window has settled on something",
        || pane_on("laptop").is_some(),
        || format!("the list holds {:?}", rows()),
    );
    assert!(
        panes(&devenv).is_empty(),
        "the untouched machine was given a pane nobody asked for: {:?}",
        panes(&devenv)
    );

    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        daemon_id: "devenv".to_string(),
        ..CreateTab::default()
    })));
    until(
        "the machine that was asked for to be given a pane",
        || panes(&devenv).len() == 1,
        || format!("the devenv holds {:?}", panes(&devenv)),
    );
    until(
        "the window to list the pane it made there",
        || panes_on("devenv").len() == 1,
        || format!("the list holds {:?}", rows()),
    );
    // The laptop is untouched by any of it: asking for one machine must not fill another.
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

/// A pane split onto another machine lands beside the pane that was split, in its tab.
///
/// What right-clicking a laptop pane and picking the devenv asks for. Naming the machine alone
/// would split whichever devenv pane its region has the keyboard on, which is in another tab
/// here - so the machine travels beside the pane in a field of its own. The first split makes
/// the devenv a part of the laptop's tab; the second splits that part rather than adding
/// another, because a tab holds one region per machine.
#[test]
fn a_split_onto_another_machine_joins_the_split_panes_tab() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();

    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let tab = tab_holding(&on_laptop).expect("the list says which tab holds each pane");
    let (before, elsewhere) = (panes(&laptop).len(), panes(&devenv).len());

    let first = made(answer(request::Payload::SplitPane(SplitPane {
        pane_id: on_laptop.clone(),
        new_pane_daemon_id: "devenv".to_string(),
        side: "right".to_string(),
        take_focus: true,
        ..SplitPane::default()
    })));
    until(
        "the new devenv pane to join the laptop's tab",
        || tab_holding(&first) == Some(tab.clone()),
        || format!("{tab} spans {:?}, and the list holds {:?}", machines_of(&tab), rows()),
    );
    assert_eq!(machines_of(&tab), ["laptop", "devenv"], "the devenv did not join at the end");
    assert_eq!(panes(&laptop).len(), before, "a split onto the devenv made a laptop pane");
    assert_eq!(panes(&devenv).len(), elsewhere + 1);
    until(
        "the keyboard to follow the new pane onto the devenv",
        || keyboard() == Some(("devenv".to_string(), first.clone())),
        || format!("the keyboard is on {:?}", keyboard()),
    );

    let second = made(answer(request::Payload::SplitPane(SplitPane {
        pane_id: on_laptop.clone(),
        new_pane_daemon_id: "devenv".to_string(),
        side: "down".to_string(),
        ..SplitPane::default()
    })));
    until(
        "the second devenv pane to join the same tab",
        || tab_holding(&second) == Some(tab.clone()),
        || format!("the list holds {:?}", rows()),
    );
    assert_eq!(
        machines_of(&tab),
        ["laptop", "devenv"],
        "a second split onto the devenv gave the tab a second devenv part"
    );
    assert_eq!(panes(&devenv).len(), elsewhere + 2);
    // Beside the pane split, not in the devenv's part: the tab is one tree over both machines.
    let wanted = format!("columns(rows({on_laptop}@laptop, {second}@devenv), {first}@devenv)");
    until(
        "the second devenv pane to go below the laptop pane it was split from",
        || drawn(&tab).as_deref() == Some(wanted.as_str()),
        || format!("{tab} is drawn as {:?}", drawn(&tab)),
    );
}

/// A pane put on another machine to the left of the pane split is drawn to its left, as a split
/// on one machine would be.
#[test]
fn a_split_onto_another_machine_to_the_left_goes_left_of_the_pane_split() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop: _laptop, devenv: _devenv } = a_window_showing_two_machines();
    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let tab = tab_holding(&on_laptop).expect("the list says which tab holds each pane");

    let joined = made(answer(request::Payload::SplitPane(SplitPane {
        pane_id: on_laptop.clone(),
        new_pane_daemon_id: "devenv".to_string(),
        side: "left".to_string(),
        take_focus: true,
        ..SplitPane::default()
    })));
    let wanted = format!("columns({joined}@devenv, {on_laptop}@laptop)");
    until(
        "the devenv's pane to be drawn left of the laptop's",
        || drawn(&tab).as_deref() == Some(wanted.as_str()),
        || format!("{tab} is drawn as {:?}", drawn(&tab)),
    );
    until(
        "the keyboard to follow the new pane onto the devenv",
        || keyboard() == Some(("devenv".to_string(), joined.clone())),
        || format!("the keyboard is on {:?}", keyboard()),
    );
}

/// `muster pane new --daemon devenv --down` from a laptop pane puts the devenv's pane below it:
/// one tab interleaving two machines, which each machine keeps whole, its own pane and the
/// other's (MIP-8, section 6). Then the two are moved about as any two panes are: dropped on
/// another's edge, and traded.
#[test]
fn a_pane_made_on_another_machine_goes_below_the_pane_it_was_made_from() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();
    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let tab = tab_holding(&on_laptop).expect("the list says which tab holds each pane");

    let below = made(answer(request::Payload::SplitPane(SplitPane {
        pane_id: on_laptop.clone(),
        new_pane_daemon_id: "devenv".to_string(),
        side: "down".to_string(),
        take_focus: true,
        ..SplitPane::default()
    })));
    let wanted = format!("rows({on_laptop}@laptop, {below}@devenv)");
    until(
        "the devenv's pane to be drawn below the laptop's",
        || drawn(&tab).as_deref() == Some(wanted.as_str()),
        || format!("{tab} is drawn as {:?}", drawn(&tab)),
    );
    until(
        "both machines to keep the tab whole",
        || {
            copy_of(&laptop, &tab) == Some(format!("[{on_laptop}/~{below}@devenv 0.50]"))
                && copy_of(&devenv, &tab) == Some(format!("[~{on_laptop}@laptop/{below} 0.50]"))
        },
        || {
            format!(
                "the laptop keeps {:?} and the devenv {:?}",
                copy_of(&laptop, &tab),
                copy_of(&devenv, &tab)
            )
        },
    );
    until(
        "the keyboard to follow the new pane onto the devenv",
        || keyboard() == Some(("devenv".to_string(), below.clone())),
        || format!("the keyboard is on {:?}", keyboard()),
    );

    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        daemon_id: "devenv".to_string(),
        pane_id: below.clone(),
        onto_pane_id: on_laptop.clone(),
        side: "right".to_string(),
        ..ArrangePane::default()
    })));
    let beside = format!("columns({on_laptop}@laptop, {below}@devenv)");
    until(
        "the devenv's pane, dropped on the laptop pane's right edge, to go to its right",
        || drawn(&tab).as_deref() == Some(beside.as_str()),
        || format!("{tab} is drawn as {:?}", drawn(&tab)),
    );

    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: below.clone(),
        onto_pane_id: on_laptop.clone(),
        ..ArrangePane::default()
    })));
    let traded = format!("columns({below}@devenv, {on_laptop}@laptop)");
    until(
        "the two panes to trade places",
        || drawn(&tab).as_deref() == Some(traded.as_str()),
        || format!("{tab} is drawn as {:?}", drawn(&tab)),
    );
    assert_eq!(tabs(&laptop), 1, "the laptop grew a tab it was not asked for");
    assert_eq!(tabs(&devenv), 2, "the devenv holds its own tab and its part of the laptop's");
}

/// A pane dropped on a row on another machine in another tab joins that tab beside the row's
/// pane, and one moved into a tab by name goes in as a column at the right of the tab's whole
/// tree, where a second machine's region used to go.
#[test]
fn a_pane_moved_into_another_machines_tab_goes_beside_the_pane_or_at_the_right() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop: _laptop, devenv: _devenv } = a_window_showing_two_machines();
    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let on_devenv = pane_on("devenv").expect("the fixture waited for it");
    let tab = tab_holding(&on_laptop).expect("the list says which tab holds each pane");

    let below = made(answer(request::Payload::SplitPane(SplitPane {
        pane_id: on_laptop.clone(),
        side: "down".to_string(),
        ..SplitPane::default()
    })));
    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: on_devenv.clone(),
        onto_pane_id: below.clone(),
        ..ArrangePane::default()
    })));
    let joined = format!("rows({on_laptop}@laptop, columns({below}@laptop, {on_devenv}@devenv))");
    until(
        "the devenv's pane to join the tab behind the row it was dropped on",
        || drawn(&tab).as_deref() == Some(joined.as_str()),
        || format!("{tab} is drawn as {:?}", drawn(&tab)),
    );

    let other = made(answer(request::Payload::CreateTab(CreateTab {
        daemon_id: "devenv".to_string(),
        ..CreateTab::default()
    })));
    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: other.clone(),
        tab_id: tab.clone(),
        ..ArrangePane::default()
    })));
    let column = format!("columns({joined}, {other}@devenv)");
    until(
        "the pane put in the tab by name to go in as a column at its right",
        || drawn(&tab).as_deref() == Some(column.as_str()),
        || format!("{tab} is drawn as {:?}", drawn(&tab)),
    );
}

/// A machine on a daemon from before a tab could span machines keeps a tab the old way: a pane
/// made there joins the tab as that machine's part, beside the others, and a pane cannot be
/// dropped beside another machine's.
#[test]
fn a_machine_too_old_for_spanning_trees_joins_a_tab_as_a_part_of_its_own() {
    let _turn = muster::testing::fresh_session();
    muster::testing::treat_as_before_spanning("devenv");
    let TwoMachines { laptop: _laptop, devenv: _devenv } = a_window_showing_two_machines();
    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let on_devenv = pane_on("devenv").expect("the fixture waited for it");
    let tab = tab_holding(&on_laptop).expect("the list says which tab holds each pane");

    let joined = made(answer(request::Payload::SplitPane(SplitPane {
        pane_id: on_laptop.clone(),
        new_pane_daemon_id: "devenv".to_string(),
        side: "down".to_string(),
        ..SplitPane::default()
    })));
    let side_by_side = format!("columns({on_laptop}@laptop, {joined}@devenv)");
    until(
        "the devenv to join the tab beside the laptop's part",
        || drawn(&tab).as_deref() == Some(side_by_side.as_str()),
        || format!("{tab} is drawn as {:?}", drawn(&tab)),
    );

    let refused = refusal(request::Payload::ArrangePane(ArrangePane {
        pane_id: on_devenv,
        onto_pane_id: on_laptop,
        side: "down".to_string(),
        ..ArrangePane::default()
    }));
    assert!(refused.contains("older than this Muster"), "the refusal said: {refused}");
}

/// A tab's tree as the window draws it, from the layouts it publishes for every tab.
fn drawn(tab: &str) -> Option<String> {
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow { layout: true })).payload
    else {
        return None;
    };
    let layout = window.layouts.into_iter().find(|layout| layout.tab_id == tab)?;
    Some(crate::view::shape(&layout.tree?))
}

/// The whole tree a daemon keeps for a tab, other machines' panes included.
fn copy_of(daemon: &Daemon, tab: &str) -> Option<String> {
    let snapshot = snapshot(daemon);
    let held = snapshot.tabs.into_iter().find(|held| held.tab == tab)?;
    Some(shape(held.root.as_ref()?))
}

/// A pane put on another machine opens at the text size of the pane that was split, as any split
/// does - not at the size of whichever pane had the keyboard, which is what `muster pane new
/// --pane X --daemon devenv` got when X was not the keyboard's pane.
#[test]
fn a_split_onto_another_machine_takes_the_split_panes_text_size() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop: _laptop, devenv: _devenv } = a_window_showing_two_machines();
    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    put_the_keyboard_on(&on_laptop);

    let sized = made(answer(request::Payload::SplitPane(SplitPane {
        pane_id: on_laptop.clone(),
        side: "right".to_string(),
        take_focus: true,
        ..SplitPane::default()
    })));
    until(
        "the keyboard to follow the split",
        || keyboard().map(|(_, pane)| pane) == Some(sized.clone()),
        || format!("the keyboard is on {:?}", keyboard()),
    );
    for _ in 0..2 {
        assert_ok(&answer(request::Payload::AdjustFontSize(AdjustFontSize {
            change: "larger".to_string(),
        })));
    }
    put_the_keyboard_on(&on_laptop);
    let size = font_size_of(&sized).expect("the sized pane is on screen");
    assert_ne!(Some(size), font_size_of(&on_laptop), "the two laptop panes are the same size");

    let joined = made(answer(request::Payload::SplitPane(SplitPane {
        pane_id: sized.clone(),
        new_pane_daemon_id: "devenv".to_string(),
        side: "right".to_string(),
        ..SplitPane::default()
    })));
    until(
        "the devenv pane to open at the split pane's size",
        || font_size_of(&joined) == Some(size),
        || {
            format!(
                "{joined} is at {:?}, the split pane at {size}, the keyboard's at {:?}",
                font_size_of(&joined),
                font_size_of(&on_laptop)
            )
        },
    );
}

/// Naming the split pane's own machine as the one to put the new pane on is an ordinary split.
///
/// The menu lists every attached machine, including the pane's own, and picking that one must
/// not make the tab grow a region.
#[test]
fn a_split_onto_the_panes_own_machine_is_an_ordinary_split() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();

    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let tab = tab_holding(&on_laptop).expect("the list says which tab holds each pane");
    let (before, elsewhere) = (panes(&laptop).len(), panes(&devenv).len());

    let split = made(answer(request::Payload::SplitPane(SplitPane {
        pane_id: on_laptop,
        new_pane_daemon_id: "laptop".to_string(),
        side: "right".to_string(),
        ..SplitPane::default()
    })));
    until(
        "the new pane to reach the laptop's tab",
        || tab_holding(&split) == Some(tab.clone()),
        || format!("the list holds {:?}", rows()),
    );
    assert_eq!(machines_of(&tab), ["laptop"]);
    assert_eq!(panes(&laptop).len(), before + 1);
    assert_eq!(panes(&devenv).len(), elsewhere);
}

/// A machine to put the new pane on that this window does not follow, refused by name - the
/// same refusal naming the machine alone gets, for the same typo.
#[test]
fn a_split_onto_a_machine_this_window_is_not_following_is_refused_by_name() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();

    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let (before, elsewhere) = (panes(&laptop).len(), panes(&devenv).len());

    let reason = refusal(request::Payload::SplitPane(SplitPane {
        pane_id: on_laptop,
        new_pane_daemon_id: "typo".to_string(),
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
    assert_eq!(panes(&laptop).len(), before, "a refused split reached the laptop anyway");
    assert_eq!(panes(&devenv).len(), elsewhere, "a refused split reached the devenv anyway");
}

/// The pane a request made, out of its answer.
fn made(response: Response) -> String {
    match response.payload {
        Some(response::Payload::Made(made)) => made.pane_id,
        other => panic!("expected the new pane's name, got {other:?}"),
    }
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

/// A pane and a machine that disagree, refused rather than resolved either way.
///
/// The CLI cannot send this - `--pane` beside `--daemon` fills `new_pane_daemon_id`, the tests
/// above - and the window cannot either, because a click reads both off one view. The socket
/// has neither guard, and this project treats it as a surface equal to the other two.
///
/// Refusing matters more than which one would have won. Acting on the machine sends the
/// laptop's pane to the devenv, and the devenv answers "holds no pane called ..., most likely
/// it closed while this was in flight" - a race blamed for a caller's mistake, and the exact
/// sentence a_2Hwef7lQT was about.
#[test]
fn a_pane_and_a_machine_that_disagree_are_refused() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();

    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let (before, elsewhere) = (panes(&laptop).len(), panes(&devenv).len());

    let reason = refusal(request::Payload::SplitPane(SplitPane {
        daemon_id: "devenv".to_string(),
        pane_id: on_laptop.clone(),
        side: "right".to_string(),
        ..SplitPane::default()
    }));

    for expected in [on_laptop.as_str(), "laptop", "devenv"] {
        assert!(
            reason.contains(expected),
            "a refusal should name the pane, the machine holding it and the one that was \
             asked for, and did not mention {expected}: {reason}"
        );
    }
    assert_eq!(panes(&laptop).len(), before, "a refused split reached the laptop anyway");
    assert_eq!(panes(&devenv).len(), elsewhere, "a refused split reached the devenv anyway");
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

/// The last list of problems, for what a config saved while the window runs could not do.
static PROBLEMS: Mutex<Option<muster::proto::ProblemsChanged>> = Mutex::new(None);

fn watch() {
    *PROBLEMS.lock().expect("a panicking reader poisoned the problems") = None;
    *ROSTER.lock().expect("a panicking reader poisoned the roster") = None;
    *VIEW.lock().expect("a panicking reader poisoned the view") = None;
    muster::ffi::muster_set_event_callback(Some(note));
}

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    match Event::decode(bytes) {
        Ok(Event { payload: Some(event::Payload::RosterChanged(roster)), .. }) => {
            *ROSTER.lock().expect("a panicking reader poisoned the roster") = Some(roster);
        }
        Ok(Event { payload: Some(event::Payload::ViewChanged(view)), .. }) => {
            *VIEW.lock().expect("a panicking reader poisoned the view") = Some(view);
        }
        Ok(Event { payload: Some(event::Payload::ProblemsChanged(problems)), .. }) => {
            *PROBLEMS.lock().expect("a panicking reader poisoned the problems") = Some(problems);
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
        .flat_map(|roster| roster.tabs.iter())
        .flat_map(|tab| {
            tab.panes
                .iter()
                .map(|pane| (pane.daemon_id.clone(), pane.given_name.clone(), pane.pane_id.clone()))
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

/// How far a pane's text is from the configured size, while the window shows it.
fn font_size_of(pane: &str) -> Option<i32> {
    let view = VIEW.lock().expect("a panicking reader poisoned the view").clone()?;
    crate::view::find_pane(&view, pane).map(|held| held.font_size_offset)
}

fn put_the_keyboard_on(pane: &str) {
    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        pane_id: pane.to_string(),
        ..FocusPane::default()
    })));
    until(
        "the keyboard to reach the pane asked for",
        || keyboard().map(|(_, held)| held).as_deref() == Some(pane),
        || format!("the keyboard is on {:?}", keyboard()),
    );
}

/// Which tab the list says holds this pane.
fn tab_holding(pane: &str) -> Option<String> {
    rows_by_tab().into_iter().find_map(|(tab, held)| (held == pane).then_some(tab))
}

/// The machines the list says a tab holds panes on, in the order its regions sit.
fn machines_of(tab: &str) -> Vec<String> {
    ROSTER
        .lock()
        .expect("a panicking reader poisoned the roster")
        .as_ref()
        .into_iter()
        .flat_map(|roster| roster.tabs.iter())
        .filter(|held| held.tab_id == tab)
        .flat_map(|held| held.daemon_ids.clone())
        .collect()
}

/// One (tab, pane) pair per pane the list holds.
fn rows_by_tab() -> Vec<(String, String)> {
    ROSTER
        .lock()
        .expect("a panicking reader poisoned the roster")
        .as_ref()
        .into_iter()
        .flat_map(|roster| roster.tabs.iter())
        .flat_map(|tab| tab.panes.iter().map(|pane| (tab.tab_id.clone(), pane.pane_id.clone())))
        .collect()
}

/// Every pane the list holds for one machine, by Muster's name for it.
fn panes_on(daemon: &str) -> Vec<String> {
    rows().into_iter().filter(|(held, ..)| held == daemon).map(|(_, _, pane)| pane).collect()
}

/// Which machine the window's keyboard is on, and which of its panes.
fn keyboard() -> Option<(String, String)> {
    let view = VIEW.lock().expect("a panicking reader poisoned the view").clone()?;
    let pane = crate::view::keyboard_pane(&view)?;
    Some((crate::view::daemon_of(&view, &pane)?, pane))
}

/// The pane the tab on screen is zoomed on, when there is one and it runs on this machine.
fn zoomed_on(daemon: &str) -> Option<String> {
    let view = VIEW.lock().expect("a panicking reader poisoned the view").clone()?;
    let zoomed = Some(view.zoomed_pane_id.clone()).filter(|pane| !pane.is_empty())?;
    (crate::view::daemon_of(&view, &zoomed)? == daemon).then_some(zoomed)
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

fn snapshot(daemon: &Daemon) -> daemon_proto::Snapshot {
    muster_harness::requests::snapshot(&mut daemon.connect())
}

fn tabs(daemon: &Daemon) -> usize {
    snapshot(daemon).tabs.len()
}

/// Every pane this daemon holds, in the order it lists them.
fn panes(daemon: &Daemon) -> Vec<String> {
    snapshot(daemon).panes.into_iter().map(|pane| pane.pane).collect()
}

fn label(daemon: &Daemon, pane: &str) -> String {
    snapshot(daemon)
        .panes
        .into_iter()
        .find(|held| held.pane == pane)
        .and_then(|held| held.label)
        .unwrap_or_default()
}

/// What a pane is showing, asked of the daemon that holds it.
fn screen(daemon: &Daemon, pane: &str) -> String {
    read_text(&mut daemon.connect(), pane, 0, 0).text
}

/// A pane name no machine holds, in a window showing two.
///
/// The request still goes - to whichever machine has the keyboard, because a pane another client
/// made on a daemon a moment ago is unknown to every mirror too, and refusing here would refuse
/// one that is about to be right. What must not survive is that machine's answer. It is correct on its own
/// terms and wrong where it lands: naming one machine while two are attached reads as "it
/// exists on the other one, and this went to the wrong machine", and blaming a race rules out
/// the far commoner cause, which is a name read off a window state that has moved on.
///
/// Its reporter spent a while proving a routing bug that does not exist (kan a_2IPgSY9mo). The
/// routing was fixed in a_2Hwef7lQT and is what the sweep above covers.
#[test]
fn a_pane_no_machine_holds_is_blamed_on_no_machine() {
    let _turn = muster::testing::fresh_session();
    let _machines = a_window_showing_two_machines();

    let on_devenv = pane_on("devenv").expect("the fixture waited for it");
    put_the_keyboard_back(&on_devenv);

    let refused = refusal(request::Payload::FocusPane(FocusPane {
        daemon_id: String::new(),
        pane_id: "p000000000".to_string(),
    }));

    assert!(
        !refused.contains("devenv") && !refused.contains("laptop"),
        "the refusal names a machine, which reads as `it is on the other one`: {refused}"
    );
    assert!(
        refused.contains("no daemon this window is following holds a pane called p000000000"),
        "the refusal does not say that no machine holds it: {refused}"
    );
    assert!(
        refused.contains("older window"),
        "the refusal does not offer a stale name as a cause, which is the commoner one and the \
         reading the old message ruled out: {refused}"
    );
    assert!(
        refused.contains("muster window"),
        "the refusal does not say how to find out what this window has: {refused}"
    );
}

/// A pane a split has only just made can be typed into at once.
///
/// The shape `P=$(muster pane new); muster pane send --pane "$P"` runs: the send arrives the
/// moment the split has answered. A split returns with its pane already in the window's mirror,
/// because the daemon's events reach the mirror before the answer to the request that caused
/// them - so a pane named in a split's answer is one this window knows, and the send is not
/// mistaken for the stale name the test above is about.
///
/// Typing into it is the sharpest version - what `pane send` is *for* is reaching a pane no
/// region shows, so it asks nothing about what is on screen.
#[test]
fn a_pane_a_split_has_only_just_made_can_be_typed_into_at_once() {
    let _turn = muster::testing::fresh_session();
    let machines = a_window_showing_two_machines();
    let laptop = &machines.laptop;

    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let made = match answer(request::Payload::SplitPane(SplitPane {
        daemon_id: String::new(),
        pane_id: on_laptop.clone(),
        side: "right".to_string(),
        ..SplitPane::default()
    }))
    .payload
    {
        Some(response::Payload::Made(made)) => made.pane_id,
        other => panic!("expected the new pane's name, got {other:?}"),
    };

    // Immediately, before waiting for anything: a window that did not yet know the pane would
    // come back with the sentence the test above asserts.
    assert_ok(&answer(request::Payload::SendToPane(SendToPane {
        daemon_id: String::new(),
        pane_id: made.clone(),
        text: SENT.to_string(),
        enter: false,
        confirm: false,
        ..Default::default()
    })));
    until(
        "the text to reach the pane that was just made",
        || screen(laptop, &made).contains(SENT),
        || format!("the new pane shows {:?}", screen(laptop, &made)),
    );
}

/// One tab holding a laptop pane beside a devenv pane, which is what MIP-2 stage four is for.
///
/// The one arrangement that crosses machines, and the one thing in Muster that no daemon can be
/// told: neither of these knows the other exists, so the grouping is one tab name held on both
/// daemons, and only Muster reads the two as one tab. What it takes is one request naming a
/// tab. The pane stays on its own machine, because it is a process, and what moves is which tab
/// it belongs to.
///
/// Asserted from the agent list rather than from either daemon, because neither daemon can
/// answer it. The laptop sees one tab with one pane and so does the devenv; only the window sees
/// one tab with two.
#[test]
fn a_tab_can_hold_panes_from_two_machines() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();

    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let on_devenv = pane_on("devenv").expect("the fixture waited for it");
    let laptop_tab = tab_holding(&on_laptop).expect("the list says which tab holds each pane");
    assert_ne!(
        tab_holding(&on_devenv),
        Some(laptop_tab.clone()),
        "the two machines' panes are in one tab before anything grouped them"
    );

    // The devenv's pane, into the laptop's tab. Named by the tab and nothing inside it, which
    // is what separates this from a drag onto a row - that orders two panes in one tree, and a
    // tree is one machine's.
    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: on_devenv.clone(),
        tab_id: laptop_tab.clone(),
        ..ArrangePane::default()
    })));

    until(
        "the devenv's pane to join the laptop's tab",
        || tab_holding(&on_devenv) == Some(laptop_tab.clone()),
        || {
            format!(
                "the laptop's tab is {laptop_tab} holding {on_laptop}, and the list holds {:?}",
                rows()
            )
        },
    );
    assert_eq!(
        tab_holding(&on_laptop),
        Some(laptop_tab.clone()),
        "grouping took the laptop's own pane out of the tab it was in"
    );

    // Each machine still holds its own pane, and one tab of its own. A grouping that had moved
    // a process would show as a pane appearing on one machine and going from the other.
    assert_eq!(panes(&laptop).len(), 1, "the laptop's pane count moved");
    assert_eq!(panes(&devenv).len(), 1, "the devenv's pane count moved");
    assert_eq!(tabs(&laptop), 1, "the laptop grew a tab it was not asked for");
    assert_eq!(tabs(&devenv), 1, "the devenv grew a tab it was not asked for");

    // And the window says the tab is on both machines, which is the answer nothing else can
    // give: it is the only place the grouping is written down.
    until(
        "the list to say the tab spans both machines",
        || machines_of(&laptop_tab) == vec!["laptop".to_string(), "devenv".to_string()],
        || format!("the tab says it is on {:?}", machines_of(&laptop_tab)),
    );
}

/// A pane moved into a named tab on another machine makes that machine a part of the tab, and
/// the part takes the tab's name at once rather than at that machine's next reconnect.
///
/// Each machine holding part of a grouped tab keeps the name itself (MIP-3, section 2), so a
/// part left nameless is what another window reading only that machine shows - and what this
/// window shows too, once the laptop's panes close and the devenv's part is all that is left.
#[test]
fn a_new_part_of_a_named_tab_takes_its_name() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop: _laptop, devenv } = a_window_showing_two_machines();
    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let on_devenv = pane_on("devenv").expect("the fixture waited for it");
    let laptop_tab = tab_holding(&on_laptop).expect("the list says which tab holds each pane");

    assert_ok(&answer(request::Payload::RenameTab(RenameTab {
        tab_id: laptop_tab.clone(),
        name: "work".to_string(),
        ..RenameTab::default()
    })));
    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: on_devenv,
        tab_id: laptop_tab.clone(),
        ..ArrangePane::default()
    })));

    let devenv_label = || {
        snapshot(&devenv)
            .tabs
            .into_iter()
            .find(|tab| tab.tab == laptop_tab)
            .and_then(|tab| tab.label)
            .and_then(|label| label.text)
    };
    until(
        "the devenv's part of the tab to be called what the tab is called",
        || devenv_label().as_deref() == Some("work"),
        || format!("the devenv's part is labelled {:?}", devenv_label()),
    );
}

/// A tab a pane cannot be put into, refused by name.
///
/// A tab name from another window, or one that closed while the request was in flight. Refused
/// rather than sent, because a daemon's answer to a pane moved into a tab it holds no part of is
/// to make that tab - so a name that means nothing would quietly produce a tab nobody asked for.
#[test]
fn a_pane_put_into_a_tab_this_window_does_not_hold_is_refused() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop: _laptop, devenv: _devenv } = a_window_showing_two_machines();
    let on_devenv = pane_on("devenv").expect("the fixture waited for it");

    let reason = refusal(request::Payload::ArrangePane(ArrangePane {
        pane_id: on_devenv,
        tab_id: "t0nesuch".to_string(),
        ..ArrangePane::default()
    }));
    assert!(
        reason.contains("t0nesuch"),
        "the refusal should name the tab that reached nothing, and said: {reason}"
    );
}

/// Evening out a tab that spans two machines divides it by what each machine holds.
///
/// The half of `--equalize` that no daemon can be asked about. Inside one machine's part the
/// dividers are the daemon's and Muster sends it a request per divider; between the parts there
/// is nothing to send, because neither daemon knows the other exists - so this is Muster's own
/// arrangement, decided here (`architecture.md`, the arrangement over regions is Muster's).
///
/// Weights by pane count rather than in halves, and that is the decision worth pinning: equal
/// panes is what evening out means, and a laptop holding two beside a devenv holding one gets
/// there by taking two thirds of the width. Split down the middle instead, the tab would come
/// back with two narrow panes beside one wide one and the answer would say it had succeeded.
#[test]
fn evening_out_a_grouped_tab_divides_it_by_what_each_machine_holds() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();

    let on_laptop = pane_on("laptop").expect("the fixture waited for it");
    let on_devenv = pane_on("devenv").expect("the fixture waited for it");
    let laptop_tab = tab_holding(&on_laptop).expect("the list says which tab holds each pane");

    // Two panes on the laptop against one on the devenv, so that halves and pane counts are
    // different answers and the assertion below can tell them apart.
    assert_ok(&answer(request::Payload::SplitPane(SplitPane {
        pane_id: on_laptop.clone(),
        side: "down".to_string(),
        ..SplitPane::default()
    })));
    until(
        "the laptop to hold the pane the split made",
        || panes_on("laptop").len() == 2,
        || format!("the laptop's part holds {:?}", panes_on("laptop")),
    );

    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: on_devenv,
        tab_id: laptop_tab.clone(),
        ..ArrangePane::default()
    })));
    until(
        "the devenv's pane to join the laptop's tab",
        || machines_of(&laptop_tab) == vec!["laptop".to_string(), "devenv".to_string()],
        || format!("the tab says it is on {:?}", machines_of(&laptop_tab)),
    );
    // Waited for rather than read once: the list says the tab is on both machines as soon as the
    // devenv's part opens, and the tree with the column in it lands with that machine's event.
    until(
        "the devenv's pane to go in as a column at the right, as wide as each pane would be",
        || machine_share().is_some_and(|share| (share - 2.0 / 3.0).abs() < 1e-4),
        || format!("the laptop's share of the tab is {:?}", machine_share()),
    );
    // Halves, so that what the equalize below changes is readable.
    assert_ok(&answer(request::Payload::SetSplitRatio(SetSplitRatio {
        tab_id: laptop_tab.clone(),
        path: Vec::new(),
        ratio: 0.5,
        ..SetSplitRatio::default()
    })));
    until(
        "the divider between the machines to sit at half",
        || machine_share().is_some_and(|share| (share - 0.5).abs() < 1e-4),
        || format!("the laptop's share of the tab is {:?}", machine_share()),
    );

    assert_ok(&answer(request::Payload::EqualizePanes(EqualizePanes {
        pane_id: on_laptop,
        ..EqualizePanes::default()
    })));
    let share = machine_share();
    assert!(
        share.is_some_and(|share| (share - 2.0 / 3.0).abs() < 1e-4),
        "the laptop holds two of the tab's three panes and the devenv one, so the parts divide \
         the width two to one - anything else leaves the panes unequal, which is the whole of \
         what was asked for: the laptop's share is {share:?}"
    );

    assert_eq!(tabs(&laptop), 1, "the laptop grew a tab it was not asked for");
    assert_eq!(tabs(&devenv), 1, "the devenv grew a tab it was not asked for");
}

/// The left machine's share of the tab on screen, which is the ratio of the split at the root
/// of its tree while the tab is drawn as its machines side by side.
fn machine_share() -> Option<f32> {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => match window.view?.tree?.node? {
            muster::proto::view_tree::Node::Split(split) => Some(split.ratio),
            muster::proto::view_tree::Node::Pane(_) => None,
        },
        other => panic!("expected a window, got {other:?}"),
    }
}

// --- driving the seam ------------------------------------------------------------------

fn answer(payload: request::Payload) -> Response {
    let request = Request::new(payload);
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

/// A pane made on one machine is drawn no callsign a pane on another holds while another is
/// free, so a callsign means one pane across every machine the window shows. The devenv is left
/// two callsigns, one of them the laptop pane's, so a draw that did not know the laptop's would
/// take it half the time.
#[test]
fn a_pane_is_drawn_no_callsign_another_machine_holds() {
    let _turn = muster::testing::fresh_session();
    let TwoMachines { laptop, devenv } = a_window_showing_two_machines();
    let callsign_of = |daemon: &Daemon, pane: &str| {
        muster_harness::requests::snapshot(&mut daemon.connect())
            .panes
            .into_iter()
            .find(|record| record.pane == pane)
            .and_then(|record| record.callsign)
            .unwrap_or_else(|| panic!("{pane} was drawn no callsign"))
    };
    let on_laptop = callsign_of(&laptop, LAPTOP_PANE);
    let on_devenv = callsign_of(&devenv, DEVENV_PANE);
    let held = [on_laptop.as_str(), on_devenv.as_str()];
    let left = muster_core::callsigns::CALLSIGNS
        .iter()
        .find(|callsign| !held.contains(callsign))
        .expect("more than two callsigns");
    let mut control = devenv.connect();
    for taken in muster_core::callsigns::CALLSIGNS {
        if *taken == on_laptop || taken == left || *taken == on_devenv {
            continue;
        }
        let join = daemon_proto::msg_request::Request::Join(daemon_proto::msg_request::Join {
            name: Some((*taken).to_string()),
            group: Some("crew".to_string()),
            pull: false,
        });
        let caller = daemon_proto::msg_request::Caller {
            as_name: Some((*taken).to_string()),
            ..Default::default()
        };
        expect(
            &mut control,
            daemon_proto::request::Service::Msg(daemon_proto::MsgRequest {
                caller: Some(caller),
                request: Some(join),
            }),
            daemon_proto::Outcome::Done,
        );
    }

    let before = panes_on("devenv");
    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        pane_id: DEVENV_PANE.to_string(),
        ..CreateTab::default()
    })));
    let made = muster_harness::until_some("the devenv to hold the pane just made", || {
        panes_on("devenv").into_iter().find(|pane| !before.contains(pane))
    });
    assert_eq!(callsign_of(&devenv, &made), *left, "drawn the laptop pane's callsign");
}

/// One Muster tab with a part on each machine: `A` on the laptop and `X` on the devenv, each
/// part's tree written as `laptop` and `devenv` say, at their generations, and ending ` zoomed P`
/// for a tree zoomed on `P`. The names are the ones the window's config gives the two machines,
/// which is what a window writes into a foreign leaf.
const SPANNING_TAB: &str = "t1span0000";
const SPAN_A: &str = "p1spanaa00";
const SPAN_B: &str = "p1spanbb00";
const SPAN_X: &str = "p1spanxx00";

fn a_tab_spanning_two_machines(
    laptop_panes: &[&str],
    laptop: (&str, u64),
    devenv: (&str, u64),
) -> TwoMachines {
    let named = |tree: &str| tree.replace('A', SPAN_A).replace('B', SPAN_B).replace('X', SPAN_X);
    let laptop = (named(laptop.0), laptop.1);
    let devenv = (named(devenv.0), devenv.1);
    let machines = two_machines_after(Devenv::HoldingAPane, |on_laptop, on_devenv| {
        let mut control = on_laptop.connect();
        make(&mut control, create(SPAN_A, in_new_tab(SPANNING_TAB)));
        for pane in &laptop_panes[1..] {
            let beside = muster_harness::requests::beside(SPAN_A, daemon_proto::Side::Down);
            make(&mut control, create(&named(pane), beside));
        }
        if laptop.1 > 0 {
            expect(&mut control, seeded(&laptop.0, laptop.1), daemon_proto::Outcome::Done);
        }
        let mut control = on_devenv.connect();
        make(&mut control, create(SPAN_X, in_new_tab(SPANNING_TAB)));
        if devenv.1 > 0 {
            expect(&mut control, seeded(&devenv.0, devenv.1), daemon_proto::Outcome::Done);
        }
    });
    until(
        "the devenv to reach the list with its pane",
        || pane_on("devenv").is_some(),
        || format!("the list holds {:?}", rows()),
    );
    machines
}

fn seeded(copy: &str, generation: u64) -> daemon_proto::request::Service {
    let (tree, zoomed) =
        copy.split_once(" zoomed ").map_or((copy, None), |(tree, zoomed)| (tree, Some(zoomed)));
    set_zoomed_tree_request(SPANNING_TAB, tree, zoomed, generation, 0)
}

/// What `daemon` holds of the spanning tab: its tree as the harness prints one, with the panes
/// called `A`, `B` and `X` again and ` zoomed P` after a zoomed one, and its layout generation.
fn spanning_copy(daemon: &Daemon) -> Option<(String, u64)> {
    let snapshot = snapshot(daemon);
    let tab = snapshot.tabs.into_iter().find(|tab| tab.tab == SPANNING_TAB)?;
    let zoomed = tab.zoomed.map(|zoomed| format!(" zoomed {zoomed}")).unwrap_or_default();
    let tree = format!("{}{zoomed}", shape(tab.root.as_ref()?))
        .replace(SPAN_A, "A")
        .replace(SPAN_B, "B")
        .replace(SPAN_X, "X");
    Some((tree, tab.layout_generation))
}

/// The devenv was unreachable while the laptop's copy was rearranged, so its copy is older. The
/// window merges the two as it attaches and catches the devenv up with the laptop's tree, at the
/// laptop's generation (MIP-8, section 2).
#[test]
fn a_lagging_machines_copy_of_a_tab_spanning_machines_is_caught_up_as_the_window_attaches() {
    let _turn = muster::testing::fresh_session();
    let machines =
        a_tab_spanning_two_machines(&["A"], ("[A|~X@devenv 0.30]", 2), ("[X|~A@laptop 0.50]", 1));
    until(
        "the devenv's copy to be caught up",
        || {
            spanning_copy(&machines.devenv)
                .as_ref()
                .map(|(tree, generation)| (tree.as_str(), *generation))
                == Some(("[~A@laptop|X 0.30]", 2))
        },
        || format!("the devenv holds {:?}", spanning_copy(&machines.devenv)),
    );
    assert_eq!(spanning_copy(&machines.laptop), Some(("[A|~X@devenv 0.30]".to_string(), 2)));
}

/// A zoom is part of the tree, so a zoom on the devenv's pane is in the laptop's copy too, though
/// the laptop does not run it. Catching the devenv up carries it there, and leaves the laptop's
/// copy where it was: a window reading the laptop's copy as unzoomed would write the zoom away.
#[test]
fn a_zoom_on_another_machines_pane_reaches_a_lagging_copy() {
    let _turn = muster::testing::fresh_session();
    let machines = a_tab_spanning_two_machines(
        &["A"],
        ("[A|~X@devenv 0.30] zoomed X", 2),
        ("[~A@laptop|X 0.50]", 1),
    );
    until(
        "the devenv's copy to be caught up, zoom and all",
        || {
            spanning_copy(&machines.devenv)
                .as_ref()
                .map(|(tree, generation)| (tree.as_str(), *generation))
                == Some(("[~A@laptop|X 0.30] zoomed X", 2))
        },
        || format!("the devenv holds {:?}", spanning_copy(&machines.devenv)),
    );
    assert_eq!(
        spanning_copy(&machines.laptop),
        Some(("[A|~X@devenv 0.30] zoomed X".to_string(), 2))
    );
}

/// Another window, whose config calls the laptop `mac`, wrote the devenv's copy last. This window
/// catches the laptop up and leaves the devenv's copy as it is: a foreign leaf's name is only the
/// writing window's, and rewriting it would have the other window rewrite it back, each a
/// generation higher (MIP-8, section 1).
#[test]
fn a_copy_calling_a_machine_by_another_windows_name_is_not_rewritten() {
    let _turn = muster::testing::fresh_session();
    let machines =
        a_tab_spanning_two_machines(&["A"], ("[A|~X@devenv 0.30]", 1), ("[~A@mac|X 0.50]", 2));
    until(
        "the laptop's copy to be caught up",
        || {
            spanning_copy(&machines.laptop)
                .as_ref()
                .map(|(tree, generation)| (tree.as_str(), *generation))
                == Some(("[A|~X@devenv 0.50]", 2))
        },
        || format!("the laptop holds {:?}", spanning_copy(&machines.laptop)),
    );
    assert_eq!(spanning_copy(&machines.devenv), Some(("[~A@mac|X 0.50]".to_string(), 2)));
}

/// A pane closing on the laptop leaves the devenv's copy naming it until a window says otherwise:
/// the laptop is connected and holds no such pane, so the window drops it and writes both copies.
#[test]
fn a_pane_closing_on_one_machine_leaves_the_other_machines_copy() {
    let _turn = muster::testing::fresh_session();
    let machines = a_tab_spanning_two_machines(
        &["A", "B"],
        ("[[A/B 0.50]|~X@devenv 0.50]", 1),
        ("[[~A@laptop/~B@laptop 0.50]|X 0.50]", 1),
    );
    expect(&mut machines.laptop.connect(), close_request(SPAN_B), daemon_proto::Outcome::Done);
    // The window writes the two copies as two requests, so the laptop's can land after the
    // devenv's: wait for both rather than reading the laptop's the moment the devenv's changes.
    until(
        "the devenv's copy to lose B, and both copies to end at one generation",
        || {
            let (laptop, devenv) =
                (spanning_copy(&machines.laptop), spanning_copy(&machines.devenv));
            devenv.as_ref().is_some_and(|(tree, _)| tree == "[~A@laptop|X 0.50]")
                && laptop.as_ref().map(|(_, generation)| *generation)
                    == devenv.as_ref().map(|(_, generation)| *generation)
        },
        || {
            format!(
                "the laptop holds {:?} and the devenv {:?}",
                spanning_copy(&machines.laptop),
                spanning_copy(&machines.devenv)
            )
        },
    );
}

/// A daemon that restarts brings its copy back whole, foreign leaf included (stage 1 of MIP-8),
/// so the window that reconnects to it finds every copy already agreeing and writes nothing.
#[test]
fn a_machine_restarted_under_a_tab_spanning_machines_needs_nothing_written_back() {
    let _turn = muster::testing::fresh_session();
    let mut machines =
        a_tab_spanning_two_machines(&["A"], ("[A|~X@devenv 0.50]", 1), ("[~A@laptop|X 0.50]", 1));
    let mut control = machines.devenv.connect();
    expect(
        &mut control,
        muster_harness::requests::session(daemon_proto::session_request::Request::Stop(
            daemon_proto::session_request::Stop {},
        )),
        daemon_proto::Outcome::Done,
    );
    machines.devenv.wait_for_exit();
    machines.devenv.restart();
    until(
        "the devenv to bring its copy back",
        || spanning_copy(&machines.devenv).is_some(),
        || format!("the devenv holds {:?}", snapshot(&machines.devenv).tabs),
    );
    until(
        "the window to reach the restarted devenv",
        || pane_on("devenv").is_some(),
        || format!("the list holds {:?}", rows()),
    );
    assert_eq!(spanning_copy(&machines.devenv), Some(("[~A@laptop|X 0.50]".to_string(), 1)));
    assert_eq!(spanning_copy(&machines.laptop), Some(("[A|~X@devenv 0.50]".to_string(), 1)));
}

/// A daemon told to stop closes every pane it holds, saying first that it is stopping, and its
/// next run brings them back. A change on the laptop while the devenv is on its way down keeps
/// the devenv's pane in the laptop's copy, because the devenv's closes are not its word that the
/// pane has gone (MIP-8, section 7).
///
/// A pane on the devenv that ignores the hang-up holds the stopping daemon, still connected, for
/// the grace it gives such a program before killing it. That is the moment a window used to
/// take the closes at their word and write the laptop's copy without `X`, which a busy machine
/// reached by chance after the devenv's own tab closed, and this test reaches every time.
#[test]
fn a_change_on_one_machine_while_another_stops_keeps_the_stopping_machines_panes() {
    let _turn = muster::testing::fresh_session();
    let mut machines =
        a_tab_spanning_two_machines(&["A"], ("[A|~X@devenv 0.50]", 1), ("[~A@laptop|X 0.50]", 1));
    let mut on_devenv = machines.devenv.connect();
    let deaf = daemon_proto::pane_request::Create {
        command: Some("trap '' HUP; echo deaf; while :; do sleep 1; done".to_string()),
        ..create("p1deaf0000", in_new_tab("t1deaf0000"))
    };
    make(&mut on_devenv, deaf);
    until(
        "the devenv's deaf pane to be ignoring the hang-up",
        || screen(&machines.devenv, "p1deaf0000").contains("deaf"),
        || format!("it shows {:?}", screen(&machines.devenv, "p1deaf0000")),
    );
    expect(
        &mut on_devenv,
        muster_harness::requests::session(daemon_proto::session_request::Request::Stop(
            daemon_proto::session_request::Stop {},
        )),
        daemon_proto::Outcome::Done,
    );
    let below = muster_harness::requests::beside(SPAN_A, daemon_proto::Side::Down);
    make(&mut machines.laptop.connect(), create(SPAN_B, below));
    until(
        "the window to list the laptop's new pane",
        || panes_on("laptop").iter().any(|pane| pane == SPAN_B),
        || format!("the list holds {:?}", rows()),
    );
    assert_eq!(
        spanning_copy(&machines.laptop),
        Some(("[[A/B 0.50]|~X@devenv 0.50]".to_string(), 1)),
        "the laptop's copy keeps the stopping devenv's pane, and nothing was written to it"
    );

    machines.devenv.wait_for_exit();
    machines.devenv.restart();
    until(
        "both copies to hold all three panes at one generation",
        || {
            let (laptop, devenv) =
                (spanning_copy(&machines.laptop), spanning_copy(&machines.devenv));
            laptop.as_ref().is_some_and(|(tree, _)| tree == "[[A/B 0.50]|~X@devenv 0.50]")
                && devenv
                    .as_ref()
                    .is_some_and(|(tree, _)| tree == "[[~A@laptop/~B@laptop 0.50]|X 0.50]")
                && laptop.map(|(_, generation)| generation)
                    == devenv.map(|(_, generation)| generation)
        },
        || {
            format!(
                "the laptop holds {:?} and the devenv {:?}",
                spanning_copy(&machines.laptop),
                spanning_copy(&machines.devenv)
            )
        },
    );
}

/// A tab grouped across two machines before either kept the whole tree is written as one, its
/// parts side by side as the window drew them, so each machine keeps the whole tab from then on
/// (MIP-8, section 9).
#[test]
fn a_tab_grouped_before_spanning_trees_is_written_as_one_tree() {
    let _turn = muster::testing::fresh_session();
    let machines = a_tab_spanning_two_machines(&["A"], ("", 0), ("", 0));
    // Whichever side the window reconciled the laptop's part on, both copies say the same.
    let written = |laptop: &str, devenv: &str| {
        spanning_copy(&machines.laptop) == Some((laptop.to_string(), 1))
            && spanning_copy(&machines.devenv) == Some((devenv.to_string(), 1))
    };
    until(
        "both copies to hold the tab's whole tree",
        || {
            written("[A|~X@devenv 0.50]", "[~A@laptop|X 0.50]")
                || written("[~X@devenv|A 0.50]", "[X|~A@laptop 0.50]")
        },
        || {
            format!(
                "the laptop holds {:?} and the devenv {:?}",
                spanning_copy(&machines.laptop),
                spanning_copy(&machines.devenv)
            )
        },
    );
}

/// A tab whose machines are all too old to hold a spanning tree stays its parts side by side, as
/// it was drawn before MIP-8, and a divider the shell names in that tree reaches what it divides:
/// the line between the machines is the window's own region weights, and a line inside the
/// laptop's part is the laptop's own divider (MIP-8, section 9).
#[test]
fn a_divider_in_a_tab_whose_machines_cannot_hold_its_tree_moves_what_it_divides() {
    let _turn = muster::testing::fresh_session();
    muster::testing::treat_as_before_spanning("laptop");
    muster::testing::treat_as_before_spanning("devenv");
    let machines = a_tab_spanning_two_machines(&["A", "B"], ("", 0), ("", 0));

    assert_ok(&answer(request::Payload::SetSplitRatio(SetSplitRatio {
        tab_id: SPANNING_TAB.to_string(),
        path: Vec::new(),
        ratio: 0.25,
        ..SetSplitRatio::default()
    })));
    until(
        "the line between the machines to move",
        || drawn_root_ratio() == Some(0.25),
        || format!("the tab is drawn split at {:?}", drawn_root_ratio()),
    );

    // Whichever side the laptop's part was reconciled on, the side holding a split is it.
    let laptop_first = matches!(
        drawn_root().and_then(|root| root.first?.node),
        Some(muster::proto::view_tree::Node::Split(_))
    );
    assert_ok(&answer(request::Payload::SetSplitRatio(SetSplitRatio {
        tab_id: SPANNING_TAB.to_string(),
        path: vec![!laptop_first],
        ratio: 0.75,
        ..SetSplitRatio::default()
    })));
    until(
        "the laptop's own divider to move",
        || spanning_copy(&machines.laptop) == Some(("[A/B 0.75]".to_string(), 0)),
        || format!("the laptop holds {:?}", spanning_copy(&machines.laptop)),
    );
    assert_eq!(spanning_copy(&machines.devenv), Some(("X".to_string(), 0)));
    assert_eq!(drawn_root_ratio(), Some(0.25), "the line between the machines moved back");
}

/// The split at the root of the spanning tab's tree, as the window draws it.
fn drawn_root() -> Option<muster::proto::ViewTreeSplit> {
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow { layout: true })).payload
    else {
        return None;
    };
    let layout = window.layouts.into_iter().find(|layout| layout.tab_id == SPANNING_TAB)?;
    match layout.tree?.node? {
        muster::proto::view_tree::Node::Split(split) => Some(*split),
        muster::proto::view_tree::Node::Pane(_) => None,
    }
}

fn drawn_root_ratio() -> Option<f32> {
    drawn_root().map(|root| root.ratio)
}

/// A change worked out from copies older than the parts hold by the time it is written, as when
/// the window's own catch-up lands first, is refused by every part. The window merges again from
/// the copies as they are now and asks once more, so a divider dragged in that moment still moves.
#[test]
fn a_change_every_part_refuses_as_stale_is_worked_out_again_and_written() {
    let _turn = muster::testing::fresh_session();
    let machines = a_tab_spanning_two_machines(
        &["A", "B"],
        ("[[A/B 0.50]|~X@devenv 0.50]", 2),
        ("[[~A@laptop/~B@laptop 0.50]|X 0.50]", 2),
    );

    muster::testing::read_next_rearrangement_stale();
    assert_ok(&answer(request::Payload::SetSplitRatio(SetSplitRatio {
        tab_id: SPANNING_TAB.to_string(),
        path: Vec::new(),
        ratio: 0.25,
        ..SetSplitRatio::default()
    })));
    until(
        "the divider to move on both copies, one generation above what they held",
        || {
            spanning_copy(&machines.laptop) == Some(("[[A/B 0.50]|~X@devenv 0.25]".to_string(), 3))
                && spanning_copy(&machines.devenv)
                    == Some(("[[~A@laptop/~B@laptop 0.50]|X 0.25]".to_string(), 3))
        },
        || {
            format!(
                "the laptop holds {:?} and the devenv {:?}",
                spanning_copy(&machines.laptop),
                spanning_copy(&machines.devenv)
            )
        },
    );
}

/// A pane made beside another machine's is made, and said to be, even when that other machine
/// refuses the tree it is then written: its machine placed it, and the next merge catches the
/// refusing part up from that machine's copy. Answering with the refusal would have a caller make
/// the pane a second time.
#[test]
fn a_pane_placed_beside_another_machines_is_made_when_that_machine_refuses_the_tree() {
    let _turn = muster::testing::fresh_session();
    let machines = a_tab_spanning_two_machines(
        &["A", "B"],
        ("[[A/B 0.50]|~X@devenv 0.50]", 1),
        ("[[~A@laptop/~B@laptop 0.50]|X 0.50]", 1),
    );

    muster::testing::refuse_next_tree_write_to("devenv");
    let below = made(answer(request::Payload::SplitPane(SplitPane {
        pane_id: SPAN_X.to_string(),
        new_pane_daemon_id: "laptop".to_string(),
        side: "down".to_string(),
        ..SplitPane::default()
    })));
    let laptop = format!("[[A/B 0.50]|[~X@devenv/{below} 0.50] 0.50]");
    let devenv = format!("[[~A@laptop/~B@laptop 0.50]|[X/~{below}@laptop 0.50] 0.50]");
    until(
        "the laptop to hold the new pane below the devenv's, and the devenv to be caught up",
        || {
            spanning_copy(&machines.laptop) == Some((laptop.clone(), 2))
                && spanning_copy(&machines.devenv) == Some((devenv.clone(), 2))
        },
        || {
            format!(
                "the laptop holds {:?} and the devenv {:?}",
                spanning_copy(&machines.laptop),
                spanning_copy(&machines.devenv)
            )
        },
    );
}

/// Once a tab's copies hold each other's panes, a daemon refuses a resize, zoom, swap or divider
/// asked of it alone, so the window works each one out on the whole tree and writes it to both
/// copies, one generation up (MIP-8, section 3). A divider here is named by the turns down the
/// whole tree, where the shell will draw it.
#[test]
fn each_change_to_a_spanning_tabs_arrangement_is_written_to_both_copies_as_one_tree() {
    let _turn = muster::testing::fresh_session();
    let machines = a_tab_spanning_two_machines(
        &["A", "B"],
        ("[[A/B 0.50]|~X@devenv 0.50]", 1),
        ("[[~A@laptop/~B@laptop 0.50]|X 0.50]", 1),
    );
    let both_at = |what: &str, laptop: &str, devenv: &str, generation: u64| {
        until(
            what,
            || {
                spanning_copy(&machines.laptop) == Some((laptop.to_string(), generation))
                    && spanning_copy(&machines.devenv) == Some((devenv.to_string(), generation))
            },
            || {
                format!(
                    "the laptop holds {:?} and the devenv {:?}",
                    spanning_copy(&machines.laptop),
                    spanning_copy(&machines.devenv)
                )
            },
        );
    };

    assert_ok(&answer(request::Payload::SetSplitRatio(SetSplitRatio {
        tab_id: SPANNING_TAB.to_string(),
        path: Vec::new(),
        ratio: 0.25,
        ..SetSplitRatio::default()
    })));
    both_at(
        "the divider between the machines to move on both copies",
        "[[A/B 0.50]|~X@devenv 0.25]",
        "[[~A@laptop/~B@laptop 0.50]|X 0.25]",
        2,
    );

    assert_ok(&answer(request::Payload::ResizePane(ResizePane {
        pane_id: SPAN_B.to_string(),
        direction: "right".to_string(),
        amount: 0.25,
        ..ResizePane::default()
    })));
    both_at(
        "a resize of the laptop's pane to move the divider between the machines",
        "[[A/B 0.50]|~X@devenv 0.50]",
        "[[~A@laptop/~B@laptop 0.50]|X 0.50]",
        3,
    );

    assert_ok(&answer(request::Payload::ZoomPane(ZoomPane {
        pane_id: SPAN_X.to_string(),
        ..ZoomPane::default()
    })));
    both_at(
        "a zoom on the devenv's pane to be on both copies",
        "[[A/B 0.50]|~X@devenv 0.50] zoomed X",
        "[[~A@laptop/~B@laptop 0.50]|X 0.50] zoomed X",
        4,
    );

    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        pane_id: SPAN_A.to_string(),
        onto_pane_id: SPAN_B.to_string(),
        ..ArrangePane::default()
    })));
    both_at(
        "the laptop's two panes to trade places on both copies",
        "[[B/A 0.50]|~X@devenv 0.50] zoomed X",
        "[[~B@laptop/~A@laptop 0.50]|X 0.50] zoomed X",
        5,
    );

    assert_ok(&answer(request::Payload::EqualizePanes(EqualizePanes {
        pane_id: SPAN_A.to_string(),
        ..EqualizePanes::default()
    })));
    both_at(
        "the three panes to be evened out across both machines, two shares to one",
        "[[B/A 0.50]|~X@devenv 0.67] zoomed X",
        "[[~B@laptop/~A@laptop 0.50]|X 0.67] zoomed X",
        6,
    );
}
