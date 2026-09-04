//! Whether the command somebody actually types drives a real window.
//!
//! The real binary, spawned as a process, against a real endpoint and a real daemon. Everything in
//! between has no other test: `argv.rs` proves a command line becomes the right request and
//! `muster-seam/tests/command.rs` proves the endpoint answers one, and neither of them would notice
//! that the binary cannot find the socket, renders nothing, or exits zero on a refusal. Each of
//! those failures looks the same from a pane: the CLI does nothing.
//!
//! One test in this binary, on purpose: the seam holds one session per process.
//!
//! The child's environment is cleared rather than inherited, and that is load-bearing rather than
//! tidy. This suite is developed inside Muster, so an inherited `MUSTER_SOCKET` would point the
//! test at the developer's own window - splitting real panes and typing into them.

use std::path::Path;
use std::process::Command;

use herdr_harness::{Daemon, until, until_file, until_some};
use muster::proto::{OpenWindow, Request, Response, Startup, request, response};
use prost::Message;
use serde_json::{Value, json};

#[test]
fn a_pane_can_drive_the_window_it_is_drawn_in() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "driven", "focus": true }));

    // Inside the daemon's own scratch directory, so the run leaves nothing behind and two runs of
    // this test in parallel cannot collide on one path.
    let socket = daemon.root().join("command.sock");
    let socket = socket.to_string_lossy().into_owned();
    accepted(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.clone(),
        ..Startup::default()
    })));
    accepted(&dispatch(request::Payload::OpenWindow(OpenWindow {})));

    // What a pane Muster made is handed. From here on this test is exactly what a program inside
    // that pane can do, and nothing else.
    let inside =
        |pane: &str| vec![("MUSTER_SOCKET", socket.clone()), ("MUSTER_PANE", pane.to_string())];

    let first = until_some("the window to describe the pane the daemon holds", || {
        let window = json_from(&run(&["window", "--json"], &inside("")));
        let pane = window["panes"].get(0)?.clone();
        Some(pane["pane"].as_str()?.to_string())
    });
    assert!(
        first.starts_with('p'),
        "the CLI is answered with Muster's own name for a pane, never the daemon's - a herdr id is \
         not unique across machines and is not addressable. Got {first:?}"
    );

    // The gesture the cards are about: an agent in a pane makes another pane below itself, names
    // it, and is told what it is called - naming no pane, because it is standing in one.
    let made = run(&["pane", "new", "--down", "--name", "🤖 A"], &inside(&first));
    assert_eq!(made.code, 0, "`muster pane new` failed: {}", made.errors);
    let made_pane = made.out.trim().to_string();
    assert!(
        made_pane.starts_with('p') && made_pane != first,
        "`muster pane new` printed {made_pane:?}, which is not the name of a new pane. A caller \
         that cannot learn the name cannot address what it just made, and the name was minted \
         inside that call."
    );

    let window =
        until_some("the window to list the pane under the name the split asked for", || {
            let window = json_from(&run(&["window", "--json"], &inside(&first)));
            let named = window["panes"].as_array()?.iter().any(|pane| {
                pane["pane"] == json!(made_pane) && pane["given_name"] == json!("🤖 A")
            });
            named.then_some(window)
        });

    // The keyboard stayed where it was, asked of the CLI's own answer rather than of the core:
    // what a script means by making a pane is `leave my cursor alone`, and the flag that says
    // otherwise defaults to off.
    assert_eq!(
        window["keyboard"],
        json!(first),
        "a split that did not ask for focus took it anyway: {window}"
    );

    let tab = a_tab_is_addressable_by_name(&window, &inside(&first));

    // The other rendering of the same answer. Read here rather than eyeballed because it is what a
    // person sees, and because a pane whose name lines up in a column is the whole reason the
    // column widths are computed at all.
    let readable = run(&["window"], &inside(&first));
    assert_eq!(readable.code, 0, "`muster window` failed: {}", readable.errors);
    for expected in [first.as_str(), made_pane.as_str(), tab.as_str(), "🤖 A", "local", "tab 1"] {
        assert!(
            readable.out.contains(expected),
            "`muster window` said nothing about {expected:?}, so somebody reading it cannot see \
             what the window holds:\n{}",
            readable.out
        );
    }
    // The keyboard, marked in the gutter of its own row rather than anywhere else on the page - so
    // this is asserted on the line and not on the output, which is the whole difference between
    // saying which pane has it and merely mentioning that some pane does.
    let has_keyboard: Vec<&str> =
        readable.out.lines().filter(|line| line.trim_start().starts_with('▸')).collect();
    assert_eq!(
        has_keyboard.len(),
        1,
        "exactly one pane has the window's keyboard, and {} rows are marked as having it:\n{}",
        has_keyboard.len(),
        readable.out
    );
    assert!(
        has_keyboard[0].contains(&first),
        "the keyboard is marked on the wrong row - it is on {first}, and the marked row is \
         {:?}",
        has_keyboard[0]
    );
    assert!(
        !readable.out.contains('\u{1b}'),
        "`muster window` wrote colour escapes into a pipe. Anything reading this output has to \
         strip them, and a pane name with one in the middle is not the name:\n{:?}",
        readable.out
    );

    // Text to a pane by name, which is an agent instructing another. Read off the filesystem
    // rather than the pane's screen: a grid wraps at its width and carries the shell's own echo of
    // the command, so reading one cannot tell `it ran` from `it is sitting at the prompt`.
    let told = daemon.root().join("told.txt");
    let sending = format!("printf 'told' > {}", told.display());
    let sent = run(&["pane", "send", "--pane", &made_pane, &sending, "--enter"], &inside(&first));
    assert_eq!(sent.code, 0, "`muster pane send` failed: {}", sent.errors);
    until_file(&told, "text sent to a pane by name to have run there");

    a_pane_can_be_read_back(&made_pane, &inside(&first));
    the_columns_are_described(&inside(&first));
    the_machines_are_named_well_enough_to_end_one(&inside(&first));
    // While both panes are still in one tab and on screen, which is what stepping walks.
    the_keyboard_steps_without_being_given_a_name(&first, &made_pane, &inside(&first));

    let (tabbed_pane, second_tab) = a_tab_is_made_and_equipped(&daemon, &inside(&first));
    a_pane_moves_to_another_tab(&made_pane, &tabbed_pane, &second_tab, &inside(&first));

    only_making_a_pane_prints_a_pane(&made_pane, &tab, &inside(&first));
    a_tab_nobody_holds_is_refused_by_name(&inside(&first));
    a_mistyped_flag_is_refused_and_says_what_was_meant(&inside(&first));
    with_no_window_to_ask_nothing_is_guessed(daemon.root());
}

/// Every machine the window is attached to, and enough about it to end one on purpose.
///
/// The census `a_28YghIUw2` asked for. herdr answers no question that gets from a process to
/// the work inside it, so pairing the two is Muster's to keep - and without it, twenty daemons
/// on a machine are twenty identical rows and the one holding somebody's live agent is picked
/// by age, which picks wrong.
fn the_machines_are_named_well_enough_to_end_one(environment: &[(&str, String)]) {
    let window = json_from(&run(&["window", "--json"], environment));
    let machines = window["daemons"].as_array().expect("a window names its machines");
    assert_eq!(machines.len(), 1, "this window is attached to one daemon: {machines:?}");
    let machine = &machines[0];

    // The socket, because it is the one thing that names *this* daemon and not the one beside
    // it - `HERDR_SOCKET_PATH=<socket> herdr server stop` is the by-hand way out.
    assert!(
        machine["socket"].as_str().is_some_and(|socket| socket.contains(".sock")),
        "a machine with no socket cannot be ended deliberately: {machine}"
    );
    // Started or adopted, which is the distinction nothing recorded. This daemon was already
    // running when the window opened, because the test started it.
    assert_eq!(machine["started_by_muster"], json!(false));
    assert_eq!(machine["host"], json!(""), "this daemon is on this machine");
    assert!(
        machine["panes"].as_u64().is_some_and(|panes| panes > 0),
        "a machine holding panes should say how many: {machine}"
    );
    // And where they are, because a count is a number people agree to and a directory is a
    // thing they recognise.
    assert!(
        machine["directories"].as_array().is_some_and(|held| !held.is_empty()),
        "a machine holding panes should say where they are: {machine}"
    );
}

/// A tab name that resolves to nothing is refused, and the refusal says which name.
///
/// The daemon is not on the command line - a tab name is unique across machines, so the window
/// finds which one holds it. That is what makes this the case worth having: the lookup either finds
/// the tab or finds nothing, and "nothing" used to be unreachable because a request with no daemon
/// was refused before the tab was ever looked for. herdr acts on whatever it has focused when it
/// does not recognize a `tab_id`, so a name passed through would move somebody else's tab.
fn a_tab_nobody_holds_is_refused_by_name(environment: &[(&str, String)]) {
    let refused = run(&["tab", "focus", "t000000000"], environment);
    assert_eq!(
        refused.code, 1,
        "a tab the window does not hold should be refused by the window and exit 1, and exited \
         {}. stderr:\n{}",
        refused.code, refused.errors
    );
    assert!(
        refused.errors.contains("t000000000"),
        "the refusal does not name the tab that went nowhere, so whoever sent it cannot tell \
         which of several names was stale:\n{}",
        refused.errors
    );
}

/// A tab can be found by name and acted on by name.
///
/// The half of this surface a script could read and not act on until tabs were named: `muster
/// window` described every tab and gave no id, so the only way to reach one was to focus a pane in
/// it - which means already knowing a pane in it.
///
/// Both halves of the read are asserted because either alone is useless: a name in `tabs[]` nothing
/// can join to, or a pane pointing at a tab the answer does not describe. Then the rename, named
/// outright rather than left to the keyboard, because a script's own tab is not the one the
/// keyboard is in. No daemon travels with any of it.
fn a_tab_is_addressable_by_name(window: &Value, environment: &[(&str, String)]) -> String {
    let tab = window["tabs"][0]["tab"].as_str().unwrap_or_default().to_string();
    assert!(
        tab.starts_with('t'),
        "the CLI is answered with Muster's own name for a tab, never the daemon's - a herdr tab id \
         is not unique across machines and is not addressable. Got {tab:?} from {window}"
    );
    assert_eq!(
        window["panes"][0]["tab"],
        json!(tab),
        "a pane says which tab holds it by name, which is the only way a pane can find its own \
         tab - nothing in its environment says: {window}"
    );

    let renamed = run(&["tab", "rename", "--tab", &tab, "🗂 the build"], environment);
    assert_eq!(renamed.code, 0, "`muster tab rename` failed: {}", renamed.errors);
    until(
        "the window to list the tab under the name it was given",
        || {
            let window = json_from(&run(&["window", "--json"], environment));
            window["tabs"].as_array().is_some_and(|tabs| {
                tabs.iter().any(|held| {
                    held["tab"] == json!(tab) && held["given_name"] == json!("🗂 the build")
                })
            })
        },
        // Asks the window again rather than remembering the last answer, because what a reader
        // needs is the row as it stands: a tab that kept its old name and a tab that has gone are
        // different bugs, and only one of them is about the rename.
        || {
            let window = json_from(&run(&["window", "--json"], environment));
            let row = window["tabs"]
                .as_array()
                .and_then(|tabs| tabs.iter().find(|held| held["tab"] == json!(tab)).cloned());
            match row {
                Some(row) => format!("the window still describes {tab} as {row}."),
                None => format!(
                    "the window describes no tab called {tab} at all, so the rename is not the \
                     first thing that went wrong here: {window}"
                ),
            }
        },
    );
    tab
}

/// What a pane has printed, which nothing else in this surface answers.
///
/// Read after `pane send`, so there is something on the pane to find. The check is that the text
/// the pane was told to print comes back - not that the answer is byte-for-byte a terminal grid,
/// which it is not: a row wraps at the pane's width and the shell echoes the command as well as
/// running it, and pinning either would be pinning herdr's rendering rather than this surface.
fn a_pane_can_be_read_back(pane: &str, environment: &[(&str, String)]) {
    let read = until_some("the pane to have printed what it was told to", || {
        let read = run(&["pane", "read", "--pane", pane], environment);
        assert_eq!(read.code, 0, "`muster pane read` failed: {}", read.errors);
        read.out.contains("told").then_some(read.out)
    });
    assert!(
        !read.contains('\u{1b}'),
        "a pane's text came back with escape codes in it, so anything matching on it has to \
         strip them first: {read:?}"
    );

    let described = json_from(&run(&["pane", "read", "--pane", pane, "--json"], environment));
    assert!(
        described["text"].as_str().is_some_and(|text| text.contains("told")),
        "`--json` and the plain answer describe different reads: {described}"
    );
    assert!(
        described["rows"].as_u64().is_some_and(|rows| rows > 0),
        "a read that came back with text says it holds no rows, so a caller cannot tell an \
         empty pane from a full one: {described}"
    );
    assert_eq!(
        described["truncated"],
        json!(false),
        "a pane that has printed two lines is reported as having more history than the read \
         reached, which is the answer that would make every read look partial: {described}"
    );

    // Fewer rows than the pane has, which is what an agent checking whether something finished
    // asks for. A ceiling rather than a count: the rows come off the bottom of the pane's grid
    // and the bottom of an idle pane is blank, so asking for one row can honestly answer with
    // none. What has to be true is that the cap held and the caller was told there is more.
    let fewer =
        json_from(&run(&["pane", "read", "--pane", pane, "--rows", "1", "--json"], environment));
    assert!(
        fewer["rows"].as_u64().is_some_and(|rows| rows <= 1),
        "a read asking for at most one row came back with more: {fewer}"
    );
    assert_eq!(
        fewer["truncated"],
        json!(true),
        "a read that stopped short of a pane's history did not say so, which is the answer that \
         would let a caller conclude it had read the whole pane: {fewer}"
    );
}

/// The parts of the tab on screen, which nothing else in the answer implies.
///
/// A person reading the plain output has the window in front of them. A script arranging one has
/// neither that nor any other way to tell how a tab is divided between the machines holding
/// panes in it: `tabs[].daemons` says which they are and not how wide each is, and a weight has
/// no other spelling at all.
fn the_columns_are_described(environment: &[(&str, String)]) {
    let window = json_from(&run(&["window", "--json"], environment));
    let regions = window["regions"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        regions.len(),
        1,
        "a tab holding panes on one machine is one column, and this answer describes {}: {window}",
        regions.len()
    );
    let region = &regions[0];
    // Which tab they divide is said once, at the top. A key on every region would be the same
    // answer repeated, since they all show the tab the window is on.
    assert_eq!(
        window["showing"], window["tabs"][0]["tab"],
        "the window says it is showing a tab the answer does not describe, so nothing can join \
         the two: {window}"
    );
    assert_eq!(
        region["daemon"], window["tabs"][0]["daemons"][0],
        "the column names a machine the tab does not say it holds panes on: {window}"
    );
    assert_eq!(
        region["keyboard"],
        json!(true),
        "no column has the keyboard, though a pane does: {window}"
    );
    assert!(
        region["weight"].as_f64().is_some_and(|weight| weight > 0.0),
        "a column with no width is a column nothing can be laid out from: {window}"
    );
}

/// The keyboard moves by a direction and by a number, neither of which names a pane.
///
/// The read half of this has printed a `place` for every pane since tabs were named, and nothing
/// consumed it - so a script could read the number the sidebar draws and had no way to act on it.
/// Stepping is the other half of the same gap: `muster window` lists panes in an order, and until
/// now walking that order meant reading it again after every move.
fn the_keyboard_steps_without_being_given_a_name(
    first: &str,
    made: &str,
    environment: &[(&str, String)],
) {
    let focused = run(&["focus", first], environment);
    assert_eq!(focused.code, 0, "`muster focus` failed: {}", focused.errors);

    let stepped = run(&["focus", "--next"], environment);
    assert_eq!(stepped.code, 0, "`muster focus --next` failed: {}", stepped.errors);
    let landed = until_some("the keyboard to land on the pane after the one it was on", || {
        let window = json_from(&run(&["window", "--json"], environment));
        let keyboard = window["keyboard"].as_str()?.to_string();
        (keyboard != first).then_some(keyboard)
    });
    assert_eq!(
        landed, made,
        "`focus --next` walks the panes in the order `muster window` lists them, and from {first} \
         the next one is {made} - it landed on {landed}"
    );

    // The number drawn beside the row, sent back as the number it is. Read out of the answer
    // rather than assumed to be 1, because a place is a position in the whole window's pane order
    // and this test is not the only thing that has made panes.
    let window = json_from(&run(&["window", "--json"], environment));
    let place = window["panes"]
        .as_array()
        .and_then(|panes| panes.iter().find(|pane| pane["pane"] == json!(first)).cloned())
        .and_then(|pane| pane["place"].as_u64())
        .unwrap_or_else(|| panic!("the window describes no pane called {first}: {window}"));

    let numbered = run(&["focus", "--place", &place.to_string()], environment);
    assert_eq!(numbered.code, 0, "`muster focus --place` failed: {}", numbered.errors);
    until(
        "the keyboard to land on the pane at the place the answer gave it",
        || json_from(&run(&["window", "--json"], environment))["keyboard"] == json!(first),
        || {
            format!(
                "place {place} is {first} in the answer, and the keyboard is on {}",
                json_from(&run(&["window", "--json"], environment))["keyboard"]
            )
        },
    );
}

/// A tab is made from a script, with something already running in it.
///
/// The other way to make a pane, and the one a script could not reach: splitting was the only
/// route, so anything that did not belong in this tab had nowhere to go. `--run` is asserted off
/// the filesystem for the reason `pane send` is - and because it is the half that proves a tab is
/// equipped the way a split is, rather than made and left bare for the caller to race.
fn a_tab_is_made_and_equipped(daemon: &Daemon, environment: &[(&str, String)]) -> (String, String) {
    let ran = daemon.root().join("tabbed.txt");
    let made = run(
        &["tab", "new", "--run", &format!("printf 'tabbed' > {}", ran.display()), "--name", "🤖 B"],
        environment,
    );
    assert_eq!(made.code, 0, "`muster tab new` failed: {}", made.errors);
    let pane = made.out.trim().to_string();
    assert!(
        pane.starts_with('p'),
        "`muster tab new` printed {pane:?}, which is not the name of a pane. It prints the pane \
         rather than the tab because the pane is what the next line of a script sends into."
    );
    until_file(&ran, "the command `tab new --run` carried to have run in the tab's pane");

    let tab = until_some("the window to describe the tab that pane is in", || {
        let window = json_from(&run(&["window", "--json"], environment));
        let held = window["panes"].as_array()?.iter().find(|held| held["pane"] == json!(pane))?;
        (held["given_name"] == json!("🤖 B")).then(|| held["tab"].as_str())?.map(str::to_string)
    });
    (pane, tab)
}

/// A pane moves to another tab, and what was running in it goes on running.
///
/// The gap the card is about: `pane new` can build any arrangement, and nothing could change one
/// that already existed. Getting the split order wrong was correctable only by closing panes and
/// making them again, which ends whatever they were doing - the opposite of what a daemon-owned
/// pane tree is for.
///
/// Asserted across tabs rather than within one because the two outcomes are told apart by where
/// the panes are, and only this one moves a pane somewhere it was not.
fn a_pane_moves_to_another_tab(
    pane: &str,
    onto: &str,
    destination: &str,
    environment: &[(&str, String)],
) {
    let moved = run(&["pane", "move", "--pane", pane, "--onto", onto], environment);
    assert_eq!(moved.code, 0, "`muster pane move` failed: {}", moved.errors);
    until(
        "the window to show the pane in the tab it was moved to",
        || {
            let window = json_from(&run(&["window", "--json"], environment));
            window["panes"].as_array().is_some_and(|panes| {
                panes
                    .iter()
                    .any(|held| held["pane"] == json!(pane) && held["tab"] == json!(destination))
            })
        },
        || {
            let window = json_from(&run(&["window", "--json"], environment));
            let row = window["panes"]
                .as_array()
                .and_then(|panes| panes.iter().find(|held| held["pane"] == json!(pane)).cloned());
            match row {
                Some(row) => {
                    format!("{pane} should be in {destination} and the window says {row}.")
                }
                None => format!(
                    "the window describes no pane called {pane} at all, so the move did not \
                     merely go to the wrong tab: {window}"
                ),
            }
        },
    );
}

/// A command that made nothing prints nothing.
///
/// `pane new` is the one command whose answer is a pane name, and that is what makes the next line
/// of a script possible. Every other command has to stay silent, or a script reading one is handed
/// a name for something it did not create - which it would then go and address. Found in the
/// running app: a rename printed the pane it had renamed, because the daemon answers a rename with
/// the same pane payload it answers a split with.
fn only_making_a_pane_prints_a_pane(pane: &str, tab: &str, environment: &[(&str, String)]) {
    for argv in [
        vec!["pane", "rename", "--pane", pane, "🤖 renamed"],
        vec!["zoom", pane],
        vec!["focus", pane],
        vec!["focus", "--next"],
        vec!["tab", "focus", tab],
        vec!["tab", "focus", "--next"],
        vec!["tab", "rename", "--tab", tab, "🗂 renamed"],
        vec!["pane", "resize", "--pane", pane, "--right"],
        vec!["sidebar"],
        vec!["font", "larger"],
        vec!["font", "reset"],
        vec!["reload"],
    ] {
        let quiet = run(&argv, environment);
        assert_eq!(quiet.code, 0, "`muster {}` failed: {}", argv.join(" "), quiet.errors);
        assert!(
            quiet.out.is_empty(),
            "`muster {}` printed {:?}. Only a command that made a pane may print one, or \
             `pane=$(muster ...)` starts working by accident on commands that made nothing.",
            argv.join(" "),
            quiet.out
        );
    }
}

/// A refusal an agent can act on, and an exit code a script can branch on.
///
/// Worth its own check because the failure is quiet: a CLI that exits zero having done nothing
/// makes a broken script look like a window that ignored it.
fn a_mistyped_flag_is_refused_and_says_what_was_meant(environment: &[(&str, String)]) {
    let refused = run(&["pane", "new", "--focused"], environment);
    assert_eq!(
        refused.code, 2,
        "a command line that could not be read should exit 2, and exited {}. stderr:\n{}",
        refused.code, refused.errors
    );
    assert!(
        refused.errors.contains("--focus"),
        "the refusal for `--focused` does not mention `--focus`, so whoever typed it has to go \
         and read the help:\n{}",
        refused.errors
    );
    assert!(
        refused.out.is_empty(),
        "a refusal was written to stdout, where a script reading an answer would find it: {:?}",
        refused.out
    );
}

/// No window, and nothing guessed about it.
///
/// The state every caller outside a pane starts in, and the one where a wrong answer is worst: a
/// CLI that picked whichever socket it found first would drive a window nobody meant.
fn with_no_window_to_ask_nothing_is_guessed(root: &Path) {
    let empty = root.join("no-muster-here");
    std::fs::create_dir_all(empty.join("state")).expect("a scratch directory can be made");
    let asked = run(&["window"], &[("MUSTER_HOME", empty.to_string_lossy().into_owned())]);
    assert_eq!(
        asked.code, 3,
        "with no window to ask this should exit 3, distinct from a refusal - a script retries one \
         and not the other. Got {} with stderr:\n{}",
        asked.code, asked.errors
    );
    assert!(
        asked.errors.contains("--socket"),
        "the refusal does not say what to do about it, and there is something to do:\n{}",
        asked.errors
    );
}

struct Ran {
    code: i32,
    out: String,
    errors: String,
}

/// The real binary, with an environment that says exactly what it is being given.
fn run(argv: &[&str], environment: &[(&str, String)]) -> Ran {
    let mut command = Command::new(env!("CARGO_BIN_EXE_muster"));
    command.args(argv).env_clear();
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("the muster binary could not be run: {error}"));
    Ran {
        code: output.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&output.stdout).into_owned(),
        errors: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn json_from(ran: &Ran) -> Value {
    assert_eq!(ran.code, 0, "`muster window --json` failed: {}", ran.errors);
    serde_json::from_str(&ran.out).unwrap_or_else(|error| {
        panic!("`muster --json` wrote something that is not JSON ({error}): {:?}", ran.out)
    })
}

/// Dispatches straight into the core, for the two things only the app can do.
///
/// Startup and open are the shell's job, so they arrive over the C ABI rather than the socket -
/// there is no endpoint to dial until the first of them has been answered.
fn dispatch(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn accepted(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
