//! What attaching settles, against a real daemon.
//!
//! Attaching is where composition meets a backend: the core is handed a pane id and has to
//! turn it into a region showing that pane's tab, with the keyboard pointed at it. Only the
//! daemon knows which tab that is, so this is the one part of composition a recorded case
//! cannot judge - `composition.json` covers everything downstream of the answer, and this
//! covers getting one.
//!
//! The refusals matter as much as the success. A window attached to a pane no daemon holds
//! renders nothing and ignores the keyboard, which is indistinguishable from every other
//! way this can go wrong and is the symptom that has cost this project the most time.
//!
//! One test in this binary, on purpose. The seam holds the session in a process global and
//! this points the whole process at a scratch daemon through the environment; a second test
//! here would race both.

use std::collections::BTreeSet;
use std::sync::Mutex;

use herdr_harness::Daemon;
use muster::proto::{
    AttachPane, ClosePane, CreateTab, Event, FocusPane, Paste, Request, Response, RosterChanged,
    SplitPane, Startup, ViewChanged, ViewNode, WindowFocus, event, request, response, view_node,
};
use prost::Message;
use serde_json::{Value, json};

#[test]
fn attaching_places_a_pane_where_the_keyboard_can_find_it() {
    let daemon = Daemon::start();
    let (first, second, finished) = a_session_with_work_already_in_it(&daemon);

    // A config file naming this daemon's socket, which is how a person points Muster at a
    // daemon it did not start - and the only way there is, since Muster runs its own herdr
    // and does not read HERDR_SOCKET_PATH.
    let config = daemon.muster_config();
    // Before startup, because that is the order the shell uses (`Sources/MusterMac/Core.swift`)
    // and the order is load-bearing: startup begins following the configured daemons, so a
    // callback registered after it misses the whole first bootstrap - every pane that already
    // existed, and whatever their agents were already doing.
    muster::ffi::muster_set_event_callback(Some(note_view));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));

    // Before any attach, so this is the state a window is in on the way up rather than one
    // it fell back to.
    let reason = refusal(request::Payload::Paste(Paste { text: "hello".to_string() }));
    assert!(
        reason.contains("no pane has this window's keyboard"),
        "input with nothing attached should say so, and said: {reason}"
    );

    let reason = refusal(request::Payload::AttachPane(AttachPane { pane_id: "w9:p9".to_string() }));
    assert!(
        reason.contains("w9:p9") && reason.contains("run `muster`"),
        "a pane no daemon holds should be refused by name, and was refused with: {reason}"
    );
    // What a person is told to do next is Muster's own, never the backend's. Asserted rather
    // than left to review because this message is read at exactly the moment somebody is
    // confused, and naming the daemon there teaches them a vocabulary Muster exists to spare
    // them (README desiderata, swappable organs).
    assert!(
        !reason.contains("herdr"),
        "a refusal a user reads should not hand them the backend's CLI: {reason}"
    );

    let one = attach(&named("first"));
    assert!(
        std::path::Path::new(&one.control_socket_path).exists(),
        "the bridge's socket is bound before attach returns, and {} is not there",
        one.control_socket_path
    );
    // A second pane in the same tab. Two things are being asserted at once because they are
    // the same mistake: a socket per process rather than per pane would hand back the path
    // it already gave out, and one bridge would be talking for both panes.
    let two = attach(&named("second"));
    assert_ne!(
        one.control_socket_path, two.control_socket_path,
        "each pane dials the core on its own socket, and both panes were given one path"
    );

    // The agent that was working before any of this began. Bootstrap says only that the
    // pane appeared, so a core that told the shell about transitions alone would leave this
    // window painting a busy agent as unknown grey until it happened to move again - and a
    // window opened onto running work is exactly when the states have to be right.
    until(
        "the working agent that predates this window to reach the shell",
        || latest_state(&named("first")).as_deref() == Some("working"),
        || format!("the core last said {:?} about {first}", latest_state(&named("first"))),
    );

    // And the one that finished before this window existed is still asking for somebody.
    // Muster saw no transition for it, so it has no observation of its own and the daemon
    // does - and a window reopened after a break reporting that nothing needs anybody is the
    // failure this whole thing exists to prevent, arrived at from the other side.
    until(
        "the agent that finished before this window to still be waiting",
        || latest_state(&named("finished")).as_deref() == Some("done"),
        || format!("the core last said {:?} about {finished}", latest_state(&named("finished"))),
    );

    // The keyboard follows the pane just attached, which is the whole of composition doing
    // its job: a region for the tab, a view-local cursor in it, and a lookup that found the
    // attachment behind it.
    //
    // Asserted on the panes rather than on the answer, because the answer is `ok` either
    // way - the seam reports that it found somewhere to send, not where. Both panes run a
    // shell, so text sent to one and not the other is visible on exactly one screen, and
    // the wrong-pane bug is the one that looks like nothing at all from here.
    //
    // A paste rather than a keystroke, because it is the intent the core hands to the
    // daemon to encode. Everything else leaves over the pane's own socket, which needs a
    // bridge process on the far end - that path has its own test, and standing one up here
    // would make this one about two things.
    // Both shells first. A pane's program is spawned when the pane is created, so text
    // pasted before its shell has drawn a prompt races the program's own first output -
    // which is how this passed alone and failed under a loaded suite.
    until(
        "both panes' shells to come up",
        || !screen(&daemon, &first).is_empty() && !screen(&daemon, &second).is_empty(),
        || {
            format!(
                "{first}: {:?}\n{second}: {:?}",
                screen(&daemon, &first),
                screen(&daemon, &second)
            )
        },
    );

    // Short enough to fit a split pane's width beside a shell prompt.
    let typed = "mstr-here";
    assert_ok(&answer(request::Payload::Paste(Paste { text: typed.to_string() })));
    until(
        "the text to appear in the pane that has the keyboard",
        || screen(&daemon, &second).contains(typed),
        || {
            format!(
                "{first}: {:?}\n{second}: {:?}",
                screen(&daemon, &first),
                screen(&daemon, &second)
            )
        },
    );
    assert!(
        !screen(&daemon, &first).contains(typed),
        "the keyboard should follow the pane just attached, and the text landed in {first} \
         as well as, or instead of, {second}"
    );

    // Both spellings, because these reach both sides: what Muster calls a pane for the
    // requests, and what the daemon calls it for the oracles.
    an_agent_finishing_unseen_waits_to_be_noticed(&daemon, &second, &named("second"));
    a_pane_no_region_shows_can_still_be_reached(&daemon);
    the_window_follows_and_drives_the_tree(&daemon, &second, &named("second"));
    a_new_tab_is_made_and_then_shown(&daemon);
    // Last, because it empties the session this whole test was built on.
    an_emptied_window_can_be_refilled(&daemon);
}

/// Closing the last pane, and getting one back.
///
/// A window with no panes was a window nobody could refill. Every request is about a pane -
/// a split splits one, a close closes one, and a new tab used to need one to name the
/// workspace to put it in - so the answer to all of them was the same refusal, and the way
/// out of an empty window was to quit and relaunch.
///
/// Driven through the daemon rather than through the core's own close, because what is under
/// test is what a window does once it is empty, and this reaches that state the way the
/// commonest one does: the daemon lost the panes and said so.
fn an_emptied_window_can_be_refilled(daemon: &Daemon) {
    for pane in panes(daemon) {
        daemon.call("pane.close", &json!({ "pane_id": pane }));
    }
    until(
        "the window to notice it has nothing left to show",
        || latest_view().is_some_and(|view| view.regions.is_empty()),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // What ⌘T sends: no daemon, no pane, no directory. There is no pane to read a workspace
    // off, so a core that only knew how to make a tab beside one has nothing to do here.
    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        daemon_id: String::new(),
        pane_id: String::new(),
        cwd: String::new(),
    })));
    until(
        "a pane to come back, and the keyboard with it",
        || {
            latest_view().is_some_and(|view| {
                view.regions.first().is_some_and(|region| !region.pane_id.is_empty())
            })
        },
        || format!("the last view the core published: {:?}", latest_view()),
    );
}

/// Making a tab, and the window moving onto it.
///
/// Two halves that fail differently. herdr's `tab.create` takes a workspace and ignores keys
/// it does not know, so a request that named the pane instead would be accepted and put the
/// tab in whichever workspace that daemon last focused - a tab that exists somewhere nobody
/// asked for. And a region cannot be pointed at the new tab when the answer arrives, because
/// the mirror has not heard of it yet and the next reconcile drops a region whose tab it does
/// not know. So the tab is remembered and shown by the event that makes it true, and this is
/// what says that actually happens.
fn a_new_tab_is_made_and_then_shown(daemon: &Daemon) {
    let before =
        latest_view().expect("the window is showing something by now").regions[0].tab_id.clone();
    let tabs_before = tab_count(daemon);

    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        daemon_id: String::new(),
        pane_id: String::new(),
        cwd: String::new(),
    })));

    until(
        "the window to move onto the tab it just asked for",
        || {
            latest_view()
                .and_then(|view| view.regions.into_iter().next())
                .is_some_and(|region| region.tab_id != before && !region.pane_id.is_empty())
        },
        || {
            format!(
                "the region still shows {before}; the last view the core published: {:?}",
                latest_view()
            )
        },
    );
    // One region, not two: a new tab is somewhere this window goes, not a second copy of the
    // window beside the first.
    assert_eq!(
        latest_view().expect("just waited for it").regions.len(),
        1,
        "a new tab opened a second region instead of moving the one that asked for it"
    );
    assert_eq!(
        tab_count(daemon),
        tabs_before + 1,
        "the daemon holds a different number of tabs than one more than before, so the tab \
         was made somewhere else or made twice"
    );
}

/// How many tabs this daemon holds, by its own account.
fn tab_count(daemon: &Daemon) -> usize {
    daemon.call("session.snapshot", &json!({}))["snapshot"]["tabs"].as_array().map_or(0, Vec::len)
}

/// The session this window opens onto, built before Muster has heard of any of it.
///
/// The ordinary case rather than a contrivance: the daemon outlives the app, so most windows
/// open onto panes whose agents have been running for a while. Everything here happens before
/// the core starts watching, so nothing below is explained by a transition it saw.
///
/// Returns two panes in one tab and one that finished in another, by the ids *this daemon*
/// knows them by - which is what the raw calls below and every oracle in this file want. What
/// Muster calls the same panes is minted when it first sees them and read back with [`named`].
fn a_session_with_work_already_in_it(daemon: &Daemon) -> (String, String, String) {
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "attach", "focus": true }));
    let first = only_pane(daemon);
    daemon.call("pane.split", &json!({ "target_pane_id": first, "direction": "right" }));
    let second = panes(daemon)
        .into_iter()
        .find(|pane| pane != &first)
        .expect("the split gives this tab a second pane");

    daemon.call(
        "pane.report_agent",
        &json!({ "pane_id": first, "agent": "probe", "source": "probe", "state": "working" }),
    );

    // One that already finished, in a tab herdr is not showing - which is how herdr comes to
    // call it `done` rather than `idle`. The second tab is created first so that it, and not
    // this pane's tab, is the daemon's active one.
    daemon.call("tab.create", &json!({ "cwd": "/tmp" }));
    let finished = panes(daemon)
        .into_iter()
        .find(|pane| pane != &first && pane != &second)
        .expect("the new tab holds a pane of its own");
    for state in ["working", "idle"] {
        daemon.call(
            "pane.report_agent",
            &json!({ "pane_id": finished, "agent": "probe", "source": "probe", "state": state }),
        );
    }
    until(
        "herdr to settle the finished agent as done, which is what it calls one nobody saw",
        || agent_status(daemon, &finished) == "done",
        || format!("herdr says {:?} about {finished}", agent_status(daemon, &finished)),
    );

    // Named so this test can find them again: Muster mints its own name for every pane and
    // nothing here can predict it. Before startup, because herdr announces a rename to nobody
    // and the bootstrap snapshot is the only thing carrying one
    // (`observations/herdr-0.8.0.md` section 16).
    for (pane, given) in [(&first, "first"), (&second, "second"), (&finished, "finished")] {
        daemon.call("pane.rename", &json!({ "pane_id": pane, "label": given }));
    }

    (first, second, finished)
}

/// Going to a pane in a tab this window is not showing.
///
/// The half of attention routing that is not a colour. Glanceable states are the floor: an
/// agent that finished or is waiting for somebody is most often on a pane no region is
/// showing, and being told about it only helps if going there works. Before this, focusing
/// such a pane was refused by name - which is a list of things you cannot reach.
fn a_pane_no_region_shows_can_still_be_reached(daemon: &Daemon) {
    let before = latest_view().expect("the window is showing something by now");
    daemon.call("tab.create", &json!({ "cwd": "/tmp" }));

    // Listed, and listed as hidden - which is the row the sidebar would draw and the state
    // this whole check is about. Waited for on the list rather than on the view, because the
    // view is the one place this pane will never appear until something surfaces it. It is
    // also where its Muster name is: nothing below wants the daemon's id for it.
    until(
        "the new tab's pane to be listed as something nothing is showing",
        || hidden_pane().is_some(),
        || format!("the list holds {:?}", roster_rows()),
    );
    let elsewhere = hidden_pane().expect("the wait above returned because there was one");

    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        daemon_id: String::new(),
        pane_id: elsewhere.clone(),
    })));

    // One region still, retargeted rather than added: switching tabs on the daemon you are
    // already looking at should not split the window in two.
    until(
        "the region to be showing the pane that was asked for",
        || {
            latest_view()
                .is_some_and(|view| view.regions.len() == 1 && view.regions[0].pane_id == elsewhere)
        },
        || format!("the last view: {:?}", latest_view()),
    );
    // And the list agrees with the window it sits beside, which is the join the sidebar
    // draws: the row that said hidden a moment ago now says it is showing.
    until(
        "the list to agree that the pane is on screen",
        || listed(&elsewhere) == Some(true),
        || format!("the list says {:?} about {elsewhere}", listed(&elsewhere)),
    );

    // Back where it started, so that what follows is about the tab it was written against.
    // Going back is the same mechanism in reverse and is worth one assertion of its own -
    // a surface that could only move away from where you were would be a trap.
    let home = before.regions[0].pane_id.clone();
    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        daemon_id: String::new(),
        pane_id: home.clone(),
    })));
    until(
        "the window to come back to the tab it started on",
        || latest_view().is_some_and(|view| view.regions[0].pane_id == home),
        || format!("the last view: {:?}", latest_view()),
    );
}

/// An agent that finishes while nobody is looking, and what happens when somebody looks.
///
/// The half of agent state no daemon can answer. herdr decides `done` from whether the
/// pane's tab is active and whether the foreground client's window has OS focus, and its
/// JSON API has no way to be told the second - so with the tab active and no client
/// reporting, herdr's answer here is `idle`. That reads as "nothing needs you" at the exact
/// moment something does, which is what this window's own focus is for.
///
/// The settling assertion is the one that cannot pass by accident. herdr never revises its
/// answer when a Muster window gains focus, because it cannot see that happen at all, so a
/// core relaying the daemon would leave this `done` forever.
fn an_agent_finishing_unseen_waits_to_be_noticed(daemon: &Daemon, backend: &str, pane: &str) {
    let report = |state: &str| {
        daemon.call(
            "pane.report_agent",
            &json!({ "pane_id": backend, "agent": "probe", "source": "probe", "state": state }),
        );
    };

    report("working");
    until(
        "the agent to reach the shell as working",
        || latest_state(pane).as_deref() == Some("working"),
        || format!("the core last said {:?} about {pane}", latest_state(pane)),
    );

    // Nothing has told the core this window is focused, which is where it starts and where a
    // window that has not yet been looked at genuinely is.
    report("idle");
    until(
        "the finished agent to be waiting for somebody",
        || latest_state(pane).as_deref() == Some("done"),
        || format!("the core last said {:?} about {pane}", latest_state(pane)),
    );

    assert_ok(&answer(request::Payload::WindowFocus(WindowFocus { focused: true })));
    until(
        "looking at the pane to settle what it was waiting for",
        || latest_state(pane).as_deref() == Some("idle"),
        || format!("the core last said {:?} about {pane}", latest_state(pane)),
    );
}

/// The view the core publishes, and the two directions it moves in.
///
/// Its own function because the test above had grown into three things; still one test,
/// because the seam holds the session in a process global and a second one would race it.
fn the_window_follows_and_drives_the_tree(daemon: &Daemon, backend: &str, second: &str) {
    // Both panes in one region, because a region shows a tab and both panes are in it. A
    // second region here would mean attaching a pane opened a second copy of its tab.
    //
    // Waited for rather than read once: a tab's tree is published on its own event, and
    // herdr publishes a one-pane tree in between while a split settles. Every assertion
    // about a tree is therefore about the one it settles on.
    until(
        "the tab's tree to settle at two leaves",
        || settled(2).is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let view = latest_view().expect("attaching publishes what the window is showing");
    assert_eq!(view.regions.len(), 1, "one tab, one region: {view:?}");
    assert_eq!(view.focused_region, view.regions[0].region_id);
    assert_eq!(view.regions[0].pane_id, second, "the keyboard is on the pane just attached");

    // A split made from another client. Nothing here asked Muster for it, which is the
    // point twice over: the view follows the daemon rather than Muster's own record of what
    // it did, and the pane it grew gets a channel although nobody attached to it. Without
    // that, a shell rendering a surface per leaf would build one it can never type into.
    daemon.call("pane.split", &json!({ "target_pane_id": backend, "direction": "right" }));
    until(
        "a third leaf, with a socket of its own, to reach the window",
        || settled(3).is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // And now the other direction: Muster asks. A split named no pane, which means the one
    // the keyboard is on - what a keybinding means. Nothing about the window is applied
    // here; the fourth leaf arrives because the daemon said so.
    assert_ok(&answer(request::Payload::SplitPane(SplitPane {
        side: "down".to_string(),
        // What a chord sends. The field defaults to false because a script means false, so a
        // test about where the keyboard lands has to say which caller it is standing in for.
        take_focus: true,
        ..SplitPane::default()
    })));
    // Where it landed, not just that something did. Every other split in this tab is a
    // column, so the pane the keyboard was on ending up under a row is the one arrangement
    // that could only have come from this request, aimed at this pane. A fourth leaf alone
    // would be satisfied by a split spelled wrong, or aimed at somebody else's pane.
    until(
        "the keyboard's pane to end up split below, which is what was asked",
        || settled(4).is_some() && parent_axis(second).as_deref() == Some("rows"),
        || {
            format!(
                "{second} sits under {:?}; the last view: {:?}",
                parent_axis(second),
                latest_view()
            )
        },
    );

    // The keyboard follows what you made, and this is the side that proves it. A leftward
    // split is two requests, so the arrangement is settled from the daemon's own answer
    // before the pane it made has been described - and every publish resolves a region
    // against the mirror's pane list, so a keyboard put there too early is taken back off.
    // What that looks like in the window is a new pane appearing unfocused while the
    // keyboard sits in the pane you split.
    let before: BTreeSet<String> =
        settled(4).expect("just waited for it").into_iter().map(|(id, _)| id).collect();
    assert_ok(&answer(request::Payload::SplitPane(SplitPane {
        side: "left".to_string(),
        // What a chord sends. The field defaults to false because a script means false, so a
        // test about where the keyboard lands has to say which caller it is standing in for.
        take_focus: true,
        ..SplitPane::default()
    })));
    until(
        "the keyboard to land on the pane the split made, on the left of the one it split",
        || {
            let landed = || -> Option<bool> {
                let panes = settled(5)?;
                let made = panes.iter().map(|(id, _)| id).find(|id| !before.contains(*id))?;
                Some(&latest_view()?.regions.into_iter().next()?.pane_id == made)
            };
            landed() == Some(true)
        },
        || {
            format!(
                "the panes before the split were {before:?}; the last view: {:?}",
                latest_view()
            )
        },
    );

    // Closing names a pane, the way a CLI would.
    let doomed = settled(5).expect("just waited for it")[0].0.clone();
    assert_ok(&answer(request::Payload::ClosePane(ClosePane {
        daemon_id: String::new(),
        pane_id: doomed.clone(),
    })));
    until(
        "the closed pane to leave the window",
        || settled(4).is_some_and(|panes| panes.iter().all(|(id, _)| id != &doomed)),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // A refusal is a refusal, not a silent no-op. Nothing this window shows holds that pane,
    // so there is no daemon to ask - which is the state a stale intent arrives in.
    let reason = refusal(request::Payload::ClosePane(ClosePane {
        daemon_id: String::new(),
        pane_id: doomed.clone(),
    }));
    assert!(
        reason.contains("is not showing that pane or tab"),
        "a request for a pane that is gone should say so, and said: {reason}"
    );
}

/// The published view's one region, once its tree has exactly `leaves` panes and each of
/// them names a socket of its own.
///
/// Everything this test asserts about a tree asks for it this way. A tree arrives on its own
/// event and a split publishes an intermediate one, so reading the latest view at an
/// arbitrary instant is asking what the window looked like mid-blink.
fn settled(count: usize) -> Option<Vec<(String, String)>> {
    let root = latest_view()?.regions.into_iter().next()?.root?;
    let panes = leaves(&root);
    let sockets: BTreeSet<&String> = panes.iter().map(|(_, socket)| socket).collect();
    (panes.len() == count && sockets.len() == count && !sockets.contains(&String::new()))
        .then_some(panes)
}

/// The last view the core published, from the callback the shell would register.
///
/// The push direction is the whole point of this message: a daemon-side split reaches the
/// window because the core said so, not because anything asked.
static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);

/// Every agent state the core has pushed, by daemon and pane.
///
/// Kept rather than counted, because the question is what the shell was last told a pane's
/// agent is doing - which is what it paints.
static STATES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// The last list of everything the daemons hold, which is what a sidebar row comes from.
static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);

extern "C" fn note_view(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    match Event::decode(bytes) {
        Ok(Event { payload: Some(event::Payload::ViewChanged(view)) }) => {
            *VIEW.lock().expect("a panicking reader poisoned the view") = Some(view);
        }
        Ok(Event { payload: Some(event::Payload::PaneStateChanged(state)) }) => {
            STATES
                .lock()
                .expect("a panicking reader poisoned the states")
                .push((state.pane_id, state.state));
        }
        Ok(Event { payload: Some(event::Payload::RosterChanged(roster)) }) => {
            *ROSTER.lock().expect("a panicking reader poisoned the roster") = Some(roster);
        }
        _ => {}
    }
}

fn latest_view() -> Option<ViewChanged> {
    VIEW.lock().expect("a panicking reader poisoned the view").clone()
}

/// Whether the list holds a row for this pane, and whether it says anything is showing it.
fn listed(pane: &str) -> Option<bool> {
    ROSTER
        .lock()
        .expect("a panicking reader poisoned the roster")
        .as_ref()?
        .daemons
        .iter()
        .flat_map(|daemon| daemon.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .find(|row| row.pane_id == pane)
        .map(|row| row.on_screen)
}

/// What Muster calls the pane somebody named `given`, once the core has said.
///
/// The roster rather than the view, because it lists every pane on every daemon - including one
/// in a tab no region is showing, which is exactly the pane this test has to be able to reach.
/// Correlated on the given name because that is the only thing about a pane that both this test
/// and the core know: the ids they each use are the two spellings this whole mechanism keeps
/// apart.
fn named(given: &str) -> String {
    until(
        &format!("the core to list a pane called {given}"),
        || roster_rows().iter().any(|(name, _)| name == given),
        || format!("the core listed {:?}", roster_rows()),
    );
    roster_rows()
        .into_iter()
        .find_map(|(name, pane)| (name == given).then_some(pane))
        .expect("the wait above returned because a row named it")
}

/// The one listed pane no region is showing, or nothing while every pane is on screen.
fn hidden_pane() -> Option<String> {
    let hidden: Vec<String> = listed_panes()
        .into_iter()
        .filter_map(|(pane, on_screen)| (!on_screen).then_some(pane))
        .collect();
    match hidden.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// Every listed pane, as the name somebody gave it and the name Muster minted for it.
fn roster_rows() -> Vec<(String, String)> {
    rows(|row| (row.given_name.clone(), row.pane_id.clone()))
}

/// Every listed pane, as Muster's name for it and whether a region is showing it.
fn listed_panes() -> Vec<(String, bool)> {
    rows(|row| (row.pane_id.clone(), row.on_screen))
}

fn rows<T>(read: impl Fn(&muster::proto::RosterPane) -> T) -> Vec<T> {
    ROSTER
        .lock()
        .expect("a panicking reader poisoned the roster")
        .as_ref()
        .into_iter()
        .flat_map(|roster| roster.daemons.iter())
        .flat_map(|daemon| daemon.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .map(read)
        .collect()
}

/// The last thing the core said about this pane's agent, if it has said anything.
fn latest_state(pane: &str) -> Option<String> {
    STATES
        .lock()
        .expect("a panicking reader poisoned the states")
        .iter()
        .rev()
        .find(|(id, _)| id == pane)
        .map(|(_, state)| state.clone())
}

/// The axis of the split this pane hangs directly off, in the published tree.
///
/// Which axis a pane's own parent has is the only thing that says a split was spelled right
/// *and* aimed right: the number of panes says neither.
fn parent_axis(pane: &str) -> Option<String> {
    fn walk(node: &ViewNode, pane: &str) -> Option<String> {
        let Some(view_node::Node::Split(split)) = &node.node else { return None };
        for child in split.first.iter().chain(split.second.iter()) {
            if let Some(view_node::Node::Pane(leaf)) = &child.node
                && leaf.pane_id == pane
            {
                return Some(split.axis.clone());
            }
            if let Some(found) = walk(child, pane) {
                return Some(found);
            }
        }
        None
    }
    walk(&latest_view()?.regions.into_iter().next()?.root?, pane)
}

/// Every pane in a tree, as (id, socket path), in reading order.
fn leaves(node: &ViewNode) -> Vec<(String, String)> {
    match &node.node {
        Some(view_node::Node::Pane(pane)) => {
            vec![(pane.pane_id.clone(), pane.control_socket_path.clone())]
        }
        Some(view_node::Node::Split(split)) => {
            split.first.iter().chain(split.second.iter()).flat_map(|child| leaves(child)).collect()
        }
        None => Vec::new(),
    }
}

fn answer(payload: request::Payload) -> Response {
    let request = Request { payload: Some(payload) };
    Response::decode(muster::dispatch(&request.encode_to_vec()).as_slice())
        .expect("the core answers every request with a decodable response")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => panic!("the core refused: {}", failure.reason),
        None => panic!("the core answered with no payload"),
        // Anything else is the core accepting: what it answers with is the request's business
        // and not this helper's.
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

fn attach(pane: &str) -> muster::proto::Attached {
    match answer(request::Payload::AttachPane(AttachPane { pane_id: pane.to_string() })).payload {
        Some(response::Payload::Attached(attached)) => attached,
        other => panic!("expected an attachment for {pane}, got {other:?}"),
    }
}

/// What herdr itself says a pane's agent is doing, as opposed to what Muster presents.
///
/// The two are deliberately allowed to differ, so a test about the difference has to be able
/// to read both.
fn agent_status(daemon: &Daemon, pane: &str) -> String {
    daemon
        .call("pane.list", &json!({}))
        .get("panes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|held| held["pane_id"].as_str() == Some(pane))
        .and_then(|held| held["agent_status"].as_str())
        .unwrap_or_default()
        .to_string()
}

fn panes(daemon: &Daemon) -> Vec<String> {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    snapshot
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("panes"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no panes in {snapshot}"))
        .iter()
        .filter_map(|pane| pane.get("pane_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn only_pane(daemon: &Daemon) -> String {
    let panes = panes(daemon);
    assert_eq!(panes.len(), 1, "a fresh workspace should hold exactly one pane: {panes:?}");
    panes[0].clone()
}

/// What a pane is showing, asked of the daemon that renders it.
///
/// A daemon renders every pane whether or not anything is attached to it, which is what
/// makes this a usable oracle here: no surface, no bridge, and a screen to read anyway.
///
/// Unwrapped, because a pane in a split is about two dozen columns wide and a line that
/// wraps comes back with a newline through the middle of it - which is a wrong answer to
/// "did this text arrive" and a confusing one to read.
fn screen(daemon: &Daemon, pane: &str) -> String {
    let read = daemon.call("pane.read", &json!({ "pane_id": pane, "source": "recent_unwrapped" }));
    read.get("read")
        .and_then(|read| read.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a pane read carries its text under `read`: {read}"))
        .to_string()
}

/// Polls a condition rather than sleeping on it, and says what it saw when it gives up.
///
/// herdr answers in under a millisecond, so a sleep long enough to be safe makes the suite
/// unpleasant and one short enough to be pleasant is flaky on a loaded machine. The third
/// argument is what turns a timeout from "something did not happen" into something readable.
fn until(what: &str, mut ready: impl FnMut() -> bool, on_failure: impl FnOnce() -> String) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("timed out after 15s waiting for {what}.\n{}", on_failure());
}
