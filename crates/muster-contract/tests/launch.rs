//! Does the app actually come up, and say so?
//!
//! Every part of Muster's launch had a test except the launch. The end-to-end input check opened
//! the control socket and spawned the bridge itself, so it proved the transport while the app's
//! own wiring never ran once - and two failures reached a user through that hole: a mistyped pane
//! name gave a blank window and a silent exit, and a bare `muster` dropped every keystroke
//! without saying so.
//!
//! These spawn the real binary against a real muster-daemon and read the run log, which is a
//! machine-readable account of what the app did. No keyboard is needed, so they say nothing about
//! what typing does; they say the app stood up and connected the things it has to connect.
//!
//! Every test is ignored, because launching an app needs a logged-in GUI session and the default
//! gate stays offline and deterministic (`docs/testing.md`). `./dev --contract` builds the app
//! and the bundle, says where they are, and runs these one at a time. Each check leaves its run
//! log under `/tmp/muster-contract/<check>/`, because that log is what a failure is read from.

use std::collections::BTreeSet;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use muster_core::composition::{Presentation, Saved, saved};
use muster_daemon_client::launch;
use muster_daemon_proto::connection;
use muster_daemon_proto::{self as proto, ConnectionKind, input_event};
use muster_harness::requests::{beside, create, in_new_tab, make, snapshot};
use muster_harness::{Daemon, Input, until, until_file, until_within};
use serde_json::Value;

/// Where each check keeps its home and its log. Short, because a socket path has to fit
/// `sockaddr_un.sun_path` and the app binds its daemon's socket and every pane's link under here.
const ROOT: &str = "/tmp/muster-contract";

/// The pane every check that stages its own daemon opens, named as a script talking to the daemon
/// directly would name one.
const PANE: &str = "p1";

/// What launchd gives a GUI process, which is what an app opened from the Dock, Finder or
/// Spotlight actually has. The bundle check runs with this rather than the developer's PATH:
/// with a real PATH it would pass on any machine that happens to have the right binaries on it,
/// which is exactly the state no user is in.
const LAUNCHD_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The shell every pane here runs, stated in Muster's config file.
///
/// Muster's file rather than the daemon's environment, and that is the point rather than a
/// detail: the app tells its daemon which shell to run, so pinning it here sends the fixture
/// through the same translation a person's settings go through, and a launch that ended up
/// running the wrong shell fails a check rather than passing quietly. `/bin/sh` for the reason
/// the harness gives its own daemons one: nobody's dotfiles, and no first-run prompt from a
/// shell meeting an empty HOME, get to decide what a pane shows.
const SHELL_CONFIG: &str = "[shell]\ncommand = \"/bin/sh\"\nmode = \"non_login\"";

/// How long after `app.ready` every pane on screen has to be typeable and painted.
///
/// Two seconds, which is what this tier has always given a healthy launch to start its bridges,
/// have them attach and paint; a healthy launch here takes about a tenth of a second. It is also
/// the number the typeable watch's five-second deadline is argued from
/// (`crates/muster-seam/src/watchdog.rs`), so raising it here moves the ground under that one.
const SETTLED_WITHIN: Duration = Duration::from_secs(2);

/// How long a launch is given to say it is ready, or to log anything else a check waits on.
///
/// Far longer than the suite's `PATIENCE`, for one launch in particular: the first of a freshly
/// built app. macOS holds the first exec of a binary it has not seen while it scans it, and the
/// first launch after a build here logged nothing for 44 s on a loaded machine - after which the
/// same binary logged `app.ready` within a tenth of a second of starting. The helpers the app
/// starts are warmed instead (`warm`), but the app is the thing under test and cannot be run
/// for nothing.
const LAUNCH_PATIENCE: Duration = Duration::from_mins(2);

/// How long an app is given to exit on SIGTERM before it is killed.
const EXIT_PATIENCE: Duration = Duration::from_secs(5);

/// How long a daemon the app started is given to stop and take its panes with it.
const STOP_PATIENCE: Duration = Duration::from_secs(5);

/// The two warnings this tier cannot avoid, and must not be declared per check.
///
/// Both are about how the tier launches Muster rather than about anything a check is doing. A
/// notification permission is granted against a bundle identifier signed by a Developer ID: a
/// binary out of `.build` has no identifier at all, and the ad-hoc signature `--bundle` applies
/// is refused outright by macOS (`docs/observations/macos-26.4.1.md`). So every check raises one
/// or the other, whichever way it launches, and no check here can ever raise neither.
///
/// Listed once because the cause is one thing. Naming them in each check's `expected` would read
/// as nine separate decisions and would quietly stop a check that genuinely wanted to assert
/// notifications work - which none can, and `a_2IneJXhwU` is where that gap is tracked.
const UNGRANTABLE_NOTIFICATIONS: [&str; 2] = ["notifications.unbundled", "notifications.refused"];

#[test]
#[ignore = "needs a logged-in GUI session: ./dev --contract"]
fn a_pane_named_at_launch_comes_up_typeable_and_painted() {
    // The whole chain: the app binds, the bridge starts, attaches, says so, and paints. Opened on
    // a pane by name, which is the argument a person has - from `muster window`, or from the
    // pane's own `$MUSTER_PANE`.
    let scratch = Scratch::new("named");
    let daemon = holding_one_pane();
    scratch.point_at(&daemon);

    let mut app = Running::start(&built_app(), &scratch, &[], &[PANE]);
    app.until_settled();
    let records = app.stop();

    expect_nothing_wrong(&records, &[]);
    expect_every_pane_painted(&records);
    let ready = expect_event(&records, "app.ready", "the app never finished launching");
    assert_eq!(
        field(ready, "typeable").as_deref(),
        Some("true"),
        "the app opened {PANE} by name and came up with nothing to type into: {ready}"
    );
}

#[test]
#[ignore = "needs a logged-in GUI session: ./dev --contract"]
fn the_app_as_it_ships_paints_every_pane() {
    // The app as it ships, which is a layout no other check here runs against.
    //
    // kan a_2Hnh3g0Y5, and the reason it reached a release. `1d7ace3` moved the daemon into
    // Contents/Library/MusterSessions.app and dropped the copy that had been going into
    // Contents/MacOS, so a released bundle had no daemon where its bridge looked for one - and
    // every pane of the 0.3.0 cask rendered nothing. The whole suite was green over it: `./dev`
    // stages the daemon beside the SwiftPM binary, which is the one layout the old rule was right
    // about, and every other check launches that binary.
    //
    // A cold start, because what depends on the layout now is the app finding the daemon it
    // ships - binary and data directory both, in the helper bundle - and starting it. The PATH
    // matters as much as the bundle: libghostty spawns each bridge, so a bridge inherits the
    // app's environment, and an app opened by Launch Services has launchd's four directories and
    // nothing else.
    let scratch = Scratch::new("bundle");
    scratch.write_config(SHELL_CONFIG);

    let mut app = Running::start(&bundled_app(), &scratch, &[("PATH", LAUNCHD_PATH)], &[]);
    app.until_settled();
    let records = app.stop();

    let starting = expect_event(
        &records,
        "daemon.starting",
        "the bundle's app started no daemon, so its window has no session behind it",
    );
    let binary = field(starting, "binary").unwrap_or_default();
    assert!(
        binary.contains(".app/Contents/Library/MusterSessions.app/Contents/MacOS/"),
        "the bundle's app started the daemon at {binary:?} rather than the one its helper bundle \
         carries. A released bundle has only that one, so this app would find no daemon to start \
         on a user's machine: check where DaemonLocation.swift looks, and where `./dev --bundle` \
         puts it."
    );
    if !has(&records, "daemon.started") {
        let refused = of(&records, "core.refused").filter_map(|record| field(record, "reason"));
        panic!(
            "the bundle's daemon at {binary} never answered, so no pane of this bundle renders \
             anything - which is what a `brew install` would produce. The app said: {:?}",
            refused.collect::<Vec<_>>()
        );
    }
    if let Some(failed) = of(&records, "bridge.attach.failed").next() {
        panic!(
            "a bridge in the assembled bundle could not attach to its pane: {failed}. Every pane \
             of this bundle renders nothing, which is what a `brew install` would produce. Either \
             the bundle's daemon did not come up where the app told the bridge to look, or the \
             bundle's bridge speaks a different protocol from the daemon beside it."
        );
    }
    expect_nothing_wrong(&records, &[]);
    expect_every_pane_painted(&records);
    expect_no_daemon_left(&scratch);
}

#[test]
#[ignore = "needs a logged-in GUI session: ./dev --contract"]
fn an_agents_state_reaches_the_window() {
    // The founding desideratum, end to end, against a real daemon.
    //
    // Everything below this is verified elsewhere - the fold by corpus cases, the subscription by
    // tests that attach their own daemon - and every one of those could pass with the app wired
    // up wrong. This is the only check that runs the whole chain the user does: daemon,
    // subscription, mirror, seam, window.
    //
    // The transitions come from the harness's fake agent, which the daemon detects the way it
    // detects a real one, from what the pane shows. Running a real agent would make this a test
    // of that agent's screen rather than of Muster.
    let scratch = Scratch::new("agent-state");
    let daemon = Daemon::start_detecting();
    make(&mut daemon.connect(), create(PANE, in_new_tab("t1")));
    daemon.run_agent(PANE);
    scratch.point_at(&daemon);

    let mut app = Running::start(&built_app(), &scratch, &[], &[PANE]);
    app.until_event(
        "mirror.bootstrap",
        "the app never built a picture of the daemon, so it knows no agent states at all",
    );
    for state in [proto::AgentState::Working, proto::AgentState::Blocked, proto::AgentState::Idle] {
        daemon.set_agent_state(PANE, state);
    }
    let log = app.log.clone();
    until(
        "the window to hear the agent go working and then blocked",
        || {
            let saw = transitions(&read_log(&log));
            saw.iter().any(|to| to == "working") && saw.iter().any(|to| to == "blocked")
        },
        || {
            format!(
                "the agent changed state three times and the window logged {:?}. Every pane \
                 showing what its agent is doing is the one thing this product is for.",
                transitions(&read_log(&log))
            )
        },
    );
    app.stop();
}

#[test]
#[ignore = "needs a logged-in GUI session: ./dev --contract"]
fn a_split_tab_becomes_splits_all_of_them_typeable() {
    // Every pane in the tab gets a surface, a bridge and something painted into it.
    //
    // The one check that runs the whole chain for more than one pane: daemon, tree, mirror,
    // composition, view, window. Everything under it is verified against a corpus or a daemon
    // with no window, and every one of those could pass with the app wiring up one surface for a
    // tab that has three - which is the failure this exists for, because a window showing one
    // pane of three looks exactly like a session with one pane in it.
    //
    // The splits are made through the daemon rather than through Muster's own keybinding,
    // because a key equivalent needs a focused window and this runs headless. What Muster's side
    // of that does is asserted in the seam's own tests against a real daemon.
    let scratch = Scratch::new("splits");
    let daemon = holding_one_pane();
    let mut control = daemon.connect();
    make(&mut control, create("p2", beside(PANE, proto::Side::Right)));
    make(&mut control, create("p3", beside(PANE, proto::Side::Down)));
    // A second tab, whose pane no region will show. Nothing below renders it, which is the
    // point: it is the pane the roster exists for, and the one a window alone loses.
    make(&mut control, create("p4", in_new_tab("t2")));
    let held = snapshot(&mut control);
    let tab: BTreeSet<String> = held
        .tabs
        .iter()
        .find(|tab| tab.tab == "t1")
        .and_then(|tab| tab.root.as_ref())
        .map(leaves)
        .unwrap_or_default();
    assert_eq!(tab.len(), 3, "the daemon was asked for three panes in t1 and holds {held:?}");
    assert_eq!(held.panes.len(), 4, "the daemon was asked for four panes and holds {held:?}");
    scratch.point_at(&daemon);

    let mut app = Running::start(&built_app(), &scratch, &[], &[PANE]);
    app.until_settled();
    let records = app.stop();

    expect_nothing_wrong(&records, &[]);
    expect_every_pane_painted(&records);

    // Asserted against the daemon's own tree, because the daemon is the oracle for "the tab
    // holds three panes", and the view published by the core is one of the things under test.
    let surfaced = values(&records, "surface.create", "pane");
    let typeable = values(&records, "pane.typeable", "pane");
    let painted = painted(&records);
    for (what, got) in
        [("built a surface for", &surfaced), ("heard a bridge attach for", &typeable)]
    {
        let missing: Vec<&String> = tab.difference(got).collect();
        assert!(
            missing.is_empty(),
            "the tab holds {tab:?} and the window {what} only {got:?}. {missing:?} are invisible \
             or deaf - which is the whole product."
        );
    }
    let unpainted: Vec<&String> = tab.difference(&painted).collect();
    assert!(unpainted.is_empty(), "{unpainted:?} never painted, so they render empty");

    // The tree, not just the count: three panes arranged flat and three panes nested are the
    // same number and a different window.
    let tree = of(&records, "view.region").last().and_then(|region| field(region, "tree"));
    let tree = tree.unwrap_or_default();
    assert!(
        tree.contains("columns(") && tree.contains("rows("),
        "the tab was split right and then down, and the core published {tree:?}. A tree with one \
         axis in it means the reconstruction collapsed a level."
    );

    // The roster, which is the half of "every agent at a glance" a window cannot carry: the
    // fourth pane is in a tab no region shows, so nothing on screen says anything about it.
    // Counts rather than names because these two discriminate on their own - four panes with
    // three of them showing is the arrangement, and any other pair means the roster is
    // describing a different session from the one the daemon holds.
    let roster = of(&records, "roster.received").last().unwrap_or_else(|| {
        panic!(
            "the window was never handed a roster, so nothing lists the panes no region is \
             showing - which is exactly the pane most likely to have finished unnoticed"
        )
    });
    let counts = (field(roster, "panes"), field(roster, "on_screen"));
    assert_eq!(
        counts,
        (Some("4".to_string()), Some("3".to_string())),
        "the daemon holds four panes with three of them on screen, and the window was handed \
         {roster}. A roster that agrees with the window instead of with the session lists \
         nothing worth surfacing."
    );
}

#[test]
#[ignore = "needs a logged-in GUI session: ./dev --contract"]
fn a_pane_that_does_not_exist_says_why() {
    // A pane that does not exist must say so rather than showing a blank window.
    //
    // A well-formed name for a pane nobody holds, because that is the mistake somebody makes: a
    // name copied from a window that has since closed the pane, or from another machine's notes.
    let scratch = Scratch::new("bad-pane");
    let daemon = holding_one_pane();
    scratch.point_at(&daemon);

    let mut app = Running::start(&built_app(), &scratch, &[], &["p1w3r07bsd"]);
    app.until_ready();
    let records = app.stop();

    // The one refusal this is about, and nothing else. A second unrelated warning here would be
    // a real finding hiding inside a check whose whole subject is a refusal.
    expect_nothing_wrong(&records, &["core.refused"]);
    let refused = expect_event(
        &records,
        "core.refused",
        "a mistyped pane name was silent, which is how it reached a user as an empty window",
    );
    assert_eq!(
        field(refused, "request").as_deref(),
        Some("attach_pane"),
        "something other than the attach was refused: {refused}"
    );
    // The name the user typed, so the log answers "which pane" without them running it again.
    assert!(
        field(refused, "reason").is_some_and(|reason| reason.contains("p1w3r07bsd")),
        "the refusal did not name the pane that was asked for: {refused}"
    );
}

#[test]
#[ignore = "needs a logged-in GUI session: ./dev --contract"]
fn a_bare_launch_opens_a_usable_window() {
    // A bare `muster` opens a usable window, which is what double-clicking sends.
    //
    // It used to render the user's shell and drop every keystroke, because the only way in was
    // to know a pane's name and pass it. This is the check that the ordinary way to open the app
    // is the ordinary way to open the app.
    let scratch = Scratch::new("bare");
    let daemon = holding_one_pane();
    scratch.point_at(&daemon);

    let mut app = Running::start(&built_app(), &scratch, &[], &[]);
    app.until_settled();
    let records = app.stop();

    expect_nothing_wrong(&records, &[]);
    expect_every_pane_painted(&records);
    let ready = expect_event(&records, "app.ready", "the app never finished launching");
    assert_eq!(
        field(ready, "typeable").as_deref(),
        Some("true"),
        "a bare `muster` came up with nothing to type into, so double-clicking the app gives a \
         window that ignores the keyboard: {ready}"
    );
}

#[test]
#[ignore = "needs a logged-in GUI session: ./dev --contract"]
fn a_clean_machine_gets_a_daemon_and_a_tab() {
    // No daemon, no config naming one, nothing: the app has to produce a window anyway.
    //
    // The first launch on a machine, and the reason Muster carries a daemon at all. Nothing is
    // running, nothing names a socket, and the app has to start its own daemon, ask it for a
    // tab, and end up with a pane somebody can type into. Every other check here but the
    // bundle's is handed a daemon that already exists.
    //
    // Stopped afterwards, since the whole point of the daemon is that it outlives the app.
    let scratch = Scratch::new("cold");
    scratch.write_config(SHELL_CONFIG);

    let mut app = Running::start(&built_app(), &scratch, &[], &[]);
    app.until_settled();
    let records = app.stop();

    expect_nothing_wrong(&records, &[]);
    expect_every_pane_painted(&records);
    expect_event(
        &records,
        "daemon.started",
        "no daemon was started, so a first launch on a clean machine shows nothing",
    );
    expect_event(
        &records,
        "tab.first.creating",
        "a daemon was started and never asked for a tab, so the window is empty",
    );
    let ready = expect_event(&records, "app.ready", "the app never finished launching");
    assert_eq!(
        field(ready, "typeable").as_deref(),
        Some("true"),
        "a cold start produced a window with nothing to type into - the daemon started, but no \
         pane reached the keyboard: {ready}"
    );
    let socket = proto::install::socket_path(&scratch.muster_home());
    assert!(
        answers(&socket),
        "the daemon does not answer on Muster's own socket at {}, which is where every later \
         launch will look for it - so the next window would start a second one",
        socket.display()
    );

    // And Muster wrote down the daemon it started, which is what `muster daemons` reads back.
    // Only a real launch takes this path - every other check points Muster at a daemon somebody
    // else started, and an adopted daemon is deliberately not written down.
    let records_directory = scratch.muster_home().join("state/daemons");
    let written = std::fs::read_dir(&records_directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .any(|record| record.contains(&*socket.to_string_lossy()));
    assert!(
        written,
        "the app started a daemon on {} and wrote no record of it under {}. Impact: `muster \
         daemons` cannot name the daemon Muster started, which is the whole of what makes ending \
         a stray one safe - the process holding somebody's live agent looks exactly like the \
         nineteen that hold nothing.",
        socket.display(),
        records_directory.display()
    );
    expect_no_daemon_left(&scratch);
}

#[test]
#[ignore = "needs a logged-in GUI session: ./dev --contract"]
fn a_pane_can_drive_the_window_it_is_drawn_in() {
    // A program inside a pane runs `muster` and is answered by the window it is drawn in.
    //
    // Three things have to be true at once and each is invisible on its own: the app has to put
    // a link to its CLI in `~/.muster/bin`, the daemon it starts has to carry that directory on
    // the PATH it hands every pane, and the pane has to be handed `MUSTER_PANE` and
    // `MUSTER_SOCKET` in the request that made it. Every one has a test of its own; nothing but
    // this says they meet.
    //
    // A cold start, because only a daemon Muster started gets that PATH. The text is typed
    // through the daemon rather than through Muster's own endpoint, so what is being proved is
    // the pane's own environment rather than a path this test already knows.
    let scratch = Scratch::new("driving");
    scratch.write_config(SHELL_CONFIG);
    let answered = scratch.root.join("answered.txt");
    // A file of its own, and written first. `answered` is read as soon as it is non-empty and
    // its last line has to be `$MUSTER_PANE`, so a second command appending to it would be a
    // race this check loses roughly one run in three - measured, by adding one.
    let census = scratch.root.join("census.txt");

    let mut app = Running::start(&built_app(), &scratch, &[], &[]);
    app.until_ready();

    let link = scratch.muster_home().join("bin/muster");
    assert!(
        link.exists(),
        "the app put no muster command at {}, so no pane can run one. A build stages the CLI \
         beside the app - try `./dev -b`.",
        link.display()
    );
    let socket = proto::install::socket_path(&scratch.muster_home());
    let held = snapshot(&mut muster_harness::Control::connect(&socket));
    let [pane] = held.panes.as_slice() else {
        panic!("a cold start should leave one pane to type into, and left {:?}", held.panes);
    };

    // Written to a file rather than read off the pane's screen: a grid wraps at its width and
    // carries the shell's own echo of the command, so reading one cannot tell an answer from the
    // question.
    let text = format!(
        "muster daemons > {census} 2>&1; muster window > {answered} 2>&1; \
         echo $MUSTER_PANE >> {answered}",
        census = census.display(),
        answered = answered.display()
    );
    Input::connect(&socket)
        .send(&pane.pane, input_event::Input::Send(input_event::Send { text, enter: true }));
    until_file(&answered, "a pane to answer what `muster window` said");
    app.stop();

    let said = std::fs::read_to_string(&answered).unwrap_or_default();
    assert!(
        said.contains("connected"),
        "a pane ran `muster window` and did not get a window back, so nothing running in a pane \
         can drive the window it is in. It got:\n{said}"
    );
    let named = said.trim().lines().last().unwrap_or_default();
    assert_eq!(
        named, pane.pane,
        "the pane read $MUSTER_PANE as {named:?} and the daemon calls it {:?}, so a program inside \
         it cannot say which pane it is and every command it sends would act on whichever pane \
         the keyboard happens to be on",
        pane.pane
    );
    assert!(
        said.lines().rev().skip(1).any(|line| line.contains(named)),
        "the window did not list {named}, which is the pane that asked - so a pane's own name is \
         not one the window answers to. It said:\n{said}"
    );

    // And the other half of the same question, from the same pane: `muster window` says what this
    // window is attached to, `muster daemons` says what is on the machine. Only a cold start can
    // prove this one, because only a daemon Muster started is written down.
    let counted = std::fs::read_to_string(&census).unwrap_or_default();
    assert!(
        counted.contains(&*socket.to_string_lossy()),
        "a pane ran `muster daemons` and did not get back the daemon this window started at {}, \
         so nothing running in a pane can find out what is on this machine before ending \
         something. It got:\n{counted}",
        socket.display()
    );
    expect_no_daemon_left(&scratch);
}

#[test]
#[ignore = "needs a logged-in GUI session: ./dev --contract"]
fn a_refused_config_opens_the_roster_it_would_have_had_nowhere_to_appear_in() {
    // A window that comes back with the roster closed still shows what is wrong with it.
    //
    // The launch-ordering bug of 2026-08-17, and the reason this check exists rather than a unit
    // one. Both layers were tested and correct alone: `Problems::has_error` had its cases and
    // `restore_presentation` had its own reasoning. Nothing owned the ORDER, so an error raised
    // during startup read `session.presentation` to decide whether to open the roster, and
    // `open()` replaced the whole of that a moment later with the saved arrangement. A window
    // that came back with the roster closed and a broken config therefore opened nothing,
    // silently - which is the exact failure the feature exists to prevent - and twenty-one tests
    // were green over it.
    //
    // Both halves have to be staged for it to bite, which is why nothing smaller catches it: a
    // config that will not parse, AND a saved arrangement with the roster closed.
    let scratch = Scratch::new("problems");
    // Unreadable in a way the parser names, rather than unreadable as a file: the point is a
    // refusal that reaches the roster, and a missing file is not a refusal at all. Before the
    // [shell] table, not after it: a bare key following a table header belongs to that table, so
    // appending this would refuse the file for an unknown key in [shell] rather than for the
    // value it names - the check would still pass, about something else.
    scratch.write_config(&format!("resize_step = \"20\"\n\n{SHELL_CONFIG}"));
    // The other half: a window remembered with the roster put away. Without this the roster is
    // open anyway and the bug cannot show.
    //
    // Written by the app's own writer, because a saved arrangement it refuses is one it ignores:
    // the window then opens as a first launch, whose roster is open, and the half this check is
    // staging is gone. A fixture that typed the format's version once went stale nine commits
    // later and staged nothing for weeks (kan a_2HSZuuZp4). `composition.restore.failed` is not
    // in the expected list below and must never be added, because a run that declares it is a
    // run asserting nothing.
    let closed =
        Saved { presentation: Presentation::default().with_sidebar(false), ..Saved::default() };
    let windows = scratch.muster_home().join("state/windows");
    std::fs::create_dir_all(&windows).expect("the check can make its arrangements directory");
    std::fs::write(windows.join("window-1.toml"), saved::to_toml(&closed))
        .expect("the check can write the arrangement it stages");

    let mut app = Running::start(&built_app(), &scratch, &[], &[]);
    app.until_settled();
    let records = app.stop();

    // The refusal is the fixture here, so it is declared. Anything else wrong is a real finding
    // and still fails.
    expect_nothing_wrong(&records, &["config.refused", "config.unreadable"]);
    let opened = expect_event(
        &records,
        "problems.sidebar.opened",
        "a broken config raised no roster, so the one thing that tells somebody their settings \
         were refused never appeared - which is what shipped on 2026-08-17",
    );
    assert!(field(opened, "impact").is_some(), "the record does not say what it cost: {opened}");
    // And the window is still a window. Opening the roster over a refused config must not come
    // at the price of the panes, which is the other way this could be "fixed".
    expect_every_pane_painted(&records);
    let ready = expect_event(&records, "app.ready", "the app never finished launching");
    assert_eq!(
        field(ready, "typeable").as_deref(),
        Some("true"),
        "a refused config left a window with nothing to type into - the settings are meant to be \
         ignored, not the session: {ready}"
    );
    expect_no_daemon_left(&scratch);
}

/// The app SwiftPM builds, which is what every check but one launches.
fn built_app() -> PathBuf {
    handed("MUSTER_CONTRACT_APP", "the app SwiftPM builds")
}

/// The same app inside `.build/muster.app`, which is a different layout and not a cosmetic one:
/// the daemon moves into a helper bundle in Contents/Library.
fn bundled_app() -> PathBuf {
    handed("MUSTER_CONTRACT_BUNDLE", "the app inside the assembled bundle")
}

fn handed(variable: &str, what: &str) -> PathBuf {
    let Some(path) = std::env::var_os(variable).map(PathBuf::from) else {
        panic!(
            "{variable} is not set, so there is no {what} to launch.\n  Impact: this check ran \
             nothing.\n  Fix: run it through ./dev --contract, which builds the app and the \
             bundle and says where they are."
        )
    };
    assert!(
        path.is_file(),
        "{variable} names {}, and nothing is there.\n  Impact: this check ran nothing.\n  Fix: \
         ./dev --contract builds it first; if it did, that step is what broke.",
        path.display()
    );
    path
}

/// A real daemon holding one tab of one pane, which is what a window has to have something to
/// show.
fn holding_one_pane() -> Daemon {
    let daemon = Daemon::start_built();
    make(&mut daemon.connect(), create(PANE, in_new_tab("t1")));
    daemon
}

/// A home of one check's own, and where its launch writes what it did.
///
/// HOME moves everything of Muster's at once - its config, its arrangements, its daemon's
/// socket, the directory of commands it puts on every pane's PATH - so a check can neither read
/// the developer's settings nor leave a window's arrangement among them. Kept after the check
/// rather than removed with it, because the run log is what a failure is read from.
#[derive(Debug)]
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(check: &str) -> Scratch {
        let scratch = Scratch { root: Path::new(ROOT).join(check) };
        // Before the delete, not after, and that ordering was one of three daemon-leak faults
        // this tier once had (a_2I7ASgulK). Removing the directory takes the last run's socket
        // with it, and a daemon whose socket is gone cannot be reached to be asked to stop:
        // `./dev --doctor` reports it as unreachable and only a signal ends it. Every check ends
        // its own daemons; this is the recovery for a run interrupted before it could.
        scratch.end_daemons();
        let _ = std::fs::remove_dir_all(&scratch.root);
        std::fs::create_dir_all(scratch.muster_home()).unwrap_or_else(|error| {
            panic!("could not make the check's home at {}: {error}", scratch.root.display())
        });
        scratch
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// Where the app keeps its own files under that home, found the way it finds them.
    fn muster_home(&self) -> PathBuf {
        self.home().join(".muster")
    }

    fn log(&self) -> PathBuf {
        self.root.join("run.jsonl")
    }

    fn write_config(&self, contents: &str) {
        let path = self.muster_home().join("config.toml");
        std::fs::write(&path, contents)
            .unwrap_or_else(|error| panic!("could not write {}: {error}", path.display()));
    }

    /// Points the app at a daemon it did not start, the way a person would: by naming its
    /// socket in their own config file, where the app will find it.
    fn point_at(&self, daemon: &Daemon) {
        let named = daemon.muster_config_with(SHELL_CONFIG);
        let contents = std::fs::read_to_string(&named).expect("the harness wrote a config");
        self.write_config(&contents);
    }

    /// Stops every daemon an app started under this home, and names any still answering.
    fn end_daemons(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.muster_home().join("daemon")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "sock"))
            .filter(|socket| {
                // A socket file outlives a daemon that was killed, so the stop failing says
                // nothing on its own; whether anything still answers is the test.
                let _ = launch::stop(socket, STOP_PATIENCE);
                answers(socket)
            })
            .collect()
    }
}

impl Drop for Scratch {
    /// A check that failed part way still ends the daemon its app started, so the leak a failure
    /// would otherwise leave is not a second thing for somebody to find.
    fn drop(&mut self) {
        self.end_daemons();
    }
}

/// Whether a daemon answers on `socket`.
fn answers(socket: &Path) -> bool {
    connection::connect(socket, ConnectionKind::Control, "muster-contract").is_ok()
}

/// Fails if a daemon the app started is still running after being asked to stop.
///
/// Checked by every check that lets the app start one, rather than left to `./dev --doctor`,
/// because a leak that only shows up in a diagnostic somebody runs when already suspicious is a
/// leak nobody finds. Four runs of this tier once left eight strays before anything noticed
/// (a_2I7ASgulK).
fn expect_no_daemon_left(scratch: &Scratch) {
    let left = scratch.end_daemons();
    assert!(
        left.is_empty(),
        "the check left {} daemon(s) running: {left:?}. Each holds a pane and outlives the run, \
         and the next run deletes the directory its socket is in - after which it can only be \
         ended with a signal, so strays accumulate silently. Its log beside the socket says why \
         it would not stop.",
        left.len()
    );
}

/// The app, launched in a process group of its own, with the log it is writing.
#[derive(Debug)]
struct Running {
    app: Child,
    log: PathBuf,
}

impl Running {
    /// Launches `app` with `arguments`, in `scratch`'s home, with `environment` on top.
    ///
    /// The environment is built rather than inherited. This may itself run in a Muster pane,
    /// whose `MUSTER_PANE`, `MUSTER_SOCKET` and `MUSTER_HOME` would point the app under test at
    /// the developer's own window; and a GUI app is handed very little anyway. What is passed on
    /// is what launchd gives one - the user, the shell, the temporary directory - plus the PATH,
    /// which the bundle check replaces with launchd's own.
    fn start(
        app: &Path,
        scratch: &Scratch,
        environment: &[(&str, &str)],
        arguments: &[&str],
    ) -> Running {
        let log = scratch.log();
        let _ = std::fs::remove_file(&log);
        let stderr = std::fs::File::create(scratch.root.join("stderr.log"))
            .expect("the check can write the app's stderr beside its log");
        warm(app);
        let mut command = Command::new(app);
        command.args(arguments).env_clear();
        for inherited in ["PATH", "USER", "LOGNAME", "SHELL", "TMPDIR"] {
            if let Some(value) = std::env::var_os(inherited) {
                command.env(inherited, value);
            }
        }
        command
            .env("HOME", scratch.home())
            .env("MUSTER_LOG_FILE", &log)
            .envs(environment.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .process_group(0);
        let app = command.spawn().unwrap_or_else(|error| {
            panic!("could not launch {}: {error}", app.display());
        });
        Running { app, log }
    }

    /// Waits until the app says it is ready, and fails if it says it could not be.
    fn until_ready(&mut self) {
        self.until_event("app.ready", "the app never finished launching");
    }

    /// Waits until the app has logged `event`, or has exited without it.
    fn until_event(&mut self, event: &str, why: &str) {
        let log = self.log.clone();
        let app = &mut self.app;
        until_within(
            &format!("the app to log {event}"),
            LAUNCH_PATIENCE,
            || {
                let records = read_log(&log);
                has(&records, event)
                    || has(&records, "app.setup.failed")
                    || app.try_wait().ok().flatten().is_some()
            },
            || events_seen(&read_log(&log)),
        );
        let records = read_log(&log);
        if let Some(failed) = of(&records, "app.setup.failed").next() {
            panic!("the app could not set itself up: {failed}");
        }
        expect_event(&records, event, why);
    }

    /// Waits for `app.ready`, and then until every pane the window was told to show has a bridge
    /// that attached and something painted - or until there is nothing left to wait for: the app
    /// came up with no pane to type into, or a bridge said it could not attach. Those are
    /// findings of their own, and each check names the one it is about.
    fn until_settled(&mut self) {
        self.until_ready();
        let log = self.log.clone();
        let app = &mut self.app;
        until_within(
            "every pane on screen to be typeable and painted",
            SETTLED_WITHIN,
            || {
                let records = read_log(&log);
                unsettled(&records).is_none()
                    || of(&records, "app.ready")
                        .any(|ready| field(ready, "typeable").as_deref() == Some("false"))
                    || has(&records, "bridge.attach.failed")
                    || app.try_wait().ok().flatten().is_some()
            },
            || unsettled(&read_log(&log)).unwrap_or_else(|| "it settled as the wait ended".into()),
        );
    }

    /// Stops the app, and returns everything the run logged.
    fn stop(mut self) -> Vec<Value> {
        self.end();
        read_log(&self.log)
    }

    /// Ends the app's whole process group: SIGTERM, then SIGKILL for an app still there.
    ///
    /// The group rather than the process, so nothing the app spawned into it outlives the check.
    /// A bridge is not among them - libghostty starts each in a session of its own, as a
    /// terminal's child - and goes when its pty does, which is when the app exits.
    fn end(&mut self) {
        if self.app.try_wait().ok().flatten().is_some() {
            return;
        }
        let group = libc::pid_t::try_from(self.app.id()).expect("a pid fits a pid_t");
        // SAFETY: killpg only sends a signal. The group is the one `process_group(0)` made for
        // this child, whose pid names it, and the child has not been reaped, so the id is not
        // one the system can have handed to anything else.
        unsafe { libc::killpg(group, libc::SIGTERM) };
        let deadline = Instant::now() + EXIT_PATIENCE;
        while Instant::now() < deadline {
            if self.app.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // SAFETY: as above; the child is still unreaped.
        unsafe { libc::killpg(group, libc::SIGKILL) };
        let _ = self.app.wait();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.end();
    }
}

/// Runs each helper the app will start, once, before the app does.
///
/// macOS holds the first exec of a binary it has not seen - which after a build is every one of
/// them - while it scans it: a freshly built muster-daemon took 15.8 s to answer here, and 8 ms
/// the next time. The app waits a minute for the daemon it starts, which covers that, but the
/// typeable watch accuses a bridge after five seconds, so every check after a rebuild would be
/// judged on how long macOS took to scan rather than on what Muster did. So each helper is run
/// first and asked for nothing it has to do: the exec is the point.
fn warm(app: &Path) {
    static WARMED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    if !WARMED.lock().unwrap_or_else(PoisonError::into_inner).insert(app.to_path_buf()) {
        return;
    }
    let beside = app.parent().expect("an executable is in a directory");
    let helpers = [
        (beside.join("muster-daemon"), Some("--version")),
        (
            beside.join("../Library/MusterSessions.app/Contents/MacOS/muster-daemon"),
            Some("--version"),
        ),
        (beside.join("muster-cli"), Some("--version")),
        // No arguments, which it answers with its usage.
        (beside.join("muster-bridge"), None),
    ];
    for (helper, argument) in helpers {
        if helper.is_file() {
            let _ = Command::new(&helper)
                .args(argument)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Every whole record the run has written so far.
///
/// Whole lines only: the app and its bridges are writing while this reads, so the last line may
/// be half there. Any finished line that does not parse fails the check, because the log is what
/// everything else here asserts on and has to survive concurrent writers.
fn read_log(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let finished = text.rfind('\n').map_or("", |end| &text[..end]);
    finished
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("an unparseable log line in {}: {line:.120} ({error})", path.display())
            })
        })
        .collect()
}

fn event(record: &Value) -> &str {
    record["event"].as_str().unwrap_or_default()
}

/// A field as text. The shell writes every field as a string, the core some as numbers or
/// booleans, and a check reads them the same way either way.
fn field(record: &Value, name: &str) -> Option<String> {
    match &record[name] {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

fn of<'a>(records: &'a [Value], name: &str) -> impl DoubleEndedIterator<Item = &'a Value> {
    records.iter().filter(move |record| event(record) == name)
}

fn has(records: &[Value], name: &str) -> bool {
    of(records, name).next().is_some()
}

/// Every value `name` takes across the records of one event.
fn values(records: &[Value], event: &str, name: &str) -> BTreeSet<String> {
    of(records, event).filter_map(|record| field(record, name)).collect()
}

fn events_seen(records: &[Value]) -> String {
    let seen: BTreeSet<&str> = records.iter().map(event).collect();
    if seen.is_empty() {
        "the app logged nothing at all".to_string()
    } else {
        format!("the app logged: {}", seen.into_iter().collect::<Vec<_>>().join(", "))
    }
}

fn expect_event<'a>(records: &'a [Value], name: &str, why: &str) -> &'a Value {
    of(records, name)
        .next()
        .unwrap_or_else(|| panic!("no `{name}` record - {why}\n    {}", events_seen(records)))
}

/// Every warning and error the app raised, unless this check declared it.
///
/// Warnings count, and that is the point. Muster's log lines carry their own impact - a
/// `pane.surface.deferred` says "this pane is blank until the core reaches its daemon" in the
/// record itself - so a run that raises one has already diagnosed a bug nobody has to think of
/// in advance. The blank-window bug of 2026-08-15 logged exactly that, at warn, while every
/// check here passed.
///
/// Declared rather than filtered by level, so that a check about a refusal says which refusal it
/// is about and a second unrelated one still fails the run.
fn expect_nothing_wrong(records: &[Value], expected: &[&str]) {
    let wrong: Vec<&Value> = records
        .iter()
        .filter(|record| matches!(record["level"].as_str(), Some("warn" | "error")))
        .filter(|record| {
            let name = event(record);
            !expected.contains(&name) && !UNGRANTABLE_NOTIFICATIONS.contains(&name)
        })
        .collect();
    let detail: Vec<String> = wrong.iter().take(5).map(ToString::to_string).collect();
    assert!(
        wrong.is_empty(),
        "{} record(s) the app itself called wrong:\n      {}{}\n    If one of these is expected \
         here, name it in this check's `expected`.",
        wrong.len(),
        detail.join("\n      "),
        if wrong.len() > 5 {
            format!("\n      ... and {} more", wrong.len() - 5)
        } else {
            String::new()
        }
    );
}

/// Every pane the window was told to show got a surface, a bridge that attached, and something
/// painted.
///
/// The gap this closes: `app.ready` with `typeable=true` is the core's answer to "is there a pane
/// the keyboard would go to", and it stays true while the window shows nothing at all. What a
/// person sees is a surface with bytes on it and a pane that answers the keyboard, and each of
/// those has its own records.
fn expect_every_pane_painted(records: &[Value]) {
    if let Some(why) = unsettled(records) {
        panic!("{why}");
    }
}

/// What is still missing before every pane on screen can be seen and typed into, if anything.
fn unsettled(records: &[Value]) -> Option<String> {
    let view: Vec<&Value> = of(records, "view.region").collect();
    if view.is_empty() {
        return Some(
            "the core never published a view, so the window was never told anything".into(),
        );
    }
    // Read out of the published tree, which is the only place the shell's own list of panes
    // appears in the log.
    let wanted: BTreeSet<String> = view
        .iter()
        .filter_map(|region| field(region, "tree"))
        .flat_map(|tree| panes_in(&tree))
        .collect();
    if wanted.is_empty() {
        return Some(format!(
            "the core published a view naming no panes at all, so the window is empty, and the \
             last thing it said it was showing was {:?}",
            view.last().and_then(|region| field(region, "tree"))
        ));
    }

    let surfaced = values(records, "surface.create", "pane");
    let missing: Vec<&String> = wanted.difference(&surfaced).collect();
    if !missing.is_empty() {
        return Some(format!(
            "the window was told to show {wanted:?} and built a surface for {surfaced:?}. \
             {missing:?} render as empty squares."
        ));
    }
    // A surface that renders and swallows the keyboard is the failure that has cost this project
    // the most time, and it is invisible without asking per pane. `pane.typeable` is the moment
    // a bridge said it attached, which is the one that decides it.
    let typeable = values(records, "pane.typeable", "pane");
    let deaf: Vec<&String> = wanted.difference(&typeable).collect();
    if !deaf.is_empty() {
        return Some(format!(
            "{deaf:?} got a surface and no bridge said it attached, so those panes swallow every \
             keystroke while looking alive"
        ));
    }
    let painted = painted(records);
    let blank: Vec<&String> = wanted.difference(&painted).collect();
    if !blank.is_empty() {
        return Some(format!(
            "{blank:?} got a surface and never painted, so they are blank squares in a window \
             that believes it is showing them"
        ));
    }
    None
}

/// The panes whose bridge wrote something to its surface.
///
/// A bridge names itself in `process` rather than repeating the pane on every record, and its
/// first write is reported at once rather than at the end of an interval.
fn painted(records: &[Value]) -> BTreeSet<String> {
    of(records, "bridge.painted")
        .filter_map(|record| record["process"].as_str()?.strip_prefix("bridge:"))
        .map(str::to_string)
        .collect()
}

/// The panes a published tree names: `columns(p1*, rows(p2*, p3*@0.5)@0.5)` names three.
///
/// A pane is its name followed by optional marks - `*` for a bound socket, a signed font offset -
/// and a split's ratio follows `@`, so a name is everything before the first of those.
fn panes_in(tree: &str) -> BTreeSet<String> {
    if tree.starts_with('(') {
        // `(not yet published)`, which names nothing.
        return BTreeSet::new();
    }
    tree.split(['(', ')', ',', ' '])
        .map(|token| token.split(['*', '+', '-', '@']).next().unwrap_or_default())
        .filter(|name| !name.is_empty() && *name != "columns" && *name != "rows")
        .map(str::to_string)
        .collect()
}

/// Every pane in a daemon's tree.
fn leaves(node: &proto::Node) -> BTreeSet<String> {
    match node.node.as_ref() {
        Some(proto::node::Node::Pane(name)) => BTreeSet::from([name.clone()]),
        Some(proto::node::Node::Split(split)) => {
            let mut panes = split.first.as_deref().map(leaves).unwrap_or_default();
            panes.extend(split.second.as_deref().map(leaves).unwrap_or_default());
            panes
        }
        None => BTreeSet::new(),
    }
}

/// Where the window heard the agent go, in order.
fn transitions(records: &[Value]) -> Vec<String> {
    of(records, "agent.state").filter_map(|record| field(record, "to")).collect()
}
