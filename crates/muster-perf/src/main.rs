//! muster-perf: what one byte, one keystroke and one event cost.
//!
//! "Fast is a feature" budgets work by frequency times cardinality, which only means
//! something if the per-unit costs are known and stay known. So this measures per unit, not
//! per run, and compares against a recorded baseline rather than against a feeling.
//!
//! Out of the default gate on purpose (docs/testing.md: a functional green is never a
//! performance claim). A timing assertion inside the gate makes the gate flaky, and a flaky
//! gate gets ignored.

use std::hint::black_box;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use muster_core::AgentState;
use muster_core::composition::{Composition, Daemon, DaemonId, Endpoint, View, ViewPane};
use muster_core::input::{
    InputEvent, InputSink, Key, KeyEvent, Modifiers, NotSent, PaneInput, PaneInputSettings,
};
use muster_core::mirror::backend::{
    AgentFacts, LayoutNode, Pane, PaneId, Snapshot, SplitAxis, Tab, TabId,
};
use muster_core::mirror::{BackendEvent, Mirror};
use muster_core::roster::Roster;
use muster_perf::{Baseline, Cost, Load, compare, context, measure, pending, table, verdict};
use muster_vt::{KeyEncoder, KeyModes, MouseEncoder, Terminal};
use prost::Message;

const USAGE: &str = "\
usage: muster-perf [--record] [--baseline <path>] [--tolerance <x>]
                   [--load <one,five,fifteen>] [--cores <total,fast>]

Measures the per-unit costs on Muster's hot paths and compares them to a baseline.

  --record       write this run's numbers as the new baseline instead of judging
  --baseline     where the baseline lives (default perf/baseline.json)
  --tolerance    how many times the recorded cost still passes, for a benchmark
                 whose baseline entry does not carry its own (default 2.0)
  --load         what the machine was doing, sampled by ./dev
  --cores        how many cores it has, and how many run at full speed";

/// How many panes a window is budgeted for.
///
/// Fifteen because that is roughly what fits on one screen before the panes stop being
/// readable, and because the desiderata name it. Nothing here is quadratic in it, and this
/// constant is how that stays true: a cost measured per pane at fifteen and divided by
/// fifteen only matches the one-pane number while it stays linear.
const BUDGETED_PANES: usize = 15;

/// The desiderata name four budgets - per byte, per event, per render - at 1 and 15 panes.
/// One of them has no code here to measure. It is printed rather than omitted, because a
/// budget nobody wrote down reads the same as a budget nobody exceeded.
const PENDING: [(&str, &str); 1] = [(
    "input-to-glyph at 15 panes",
    "everything Muster does per pane is measured above and is linear. What is left is what \
     libghostty's own renderer threads do with fifteen surfaces, which no offline benchmark \
     can see - it needs a real window in front of a real daemon, which is ./dev --latency.",
)];

/// How far each cost may drift before it counts as a regression, written into the baseline
/// when one is recorded.
///
/// One number for the whole file treated a benchmark that is arithmetic over a byte array and
/// one that binds fifteen unix sockets as equally noisy, which they are not - and the number
/// had to be loose enough for the noisiest of them, which left the quiet ones ungated. A 2.00x
/// tolerance on a per-byte parse passes a change that doubles the cost of every byte a pane
/// prints.
///
/// The numbers below come from eight consecutive runs of this harness on one unchanged tree,
/// on a ten-core laptop sitting at a load average of about ten. Each benchmark's own
/// worst-to-best spread across those runs was:
///
/// ```text
/// mirror.apply 1.06x   roster.build 1.06x   seam.dispatch 1.09x   view.build 1.09x
/// pane.encoder 1.27x   pane.channel 1.42x
/// ```
///
/// The per-byte parse and the per-key path measured 1.06x and 1.14x in the shapes they had
/// then. So the arithmetic-over-a-buffer benchmarks are stable to within a tenth even on a busy
/// machine, and the two that touch the OS are not. A single 2.00x covered the noisiest of them
/// and left the quiet ones ungated.
///
/// Recorded into the baseline file rather than applied from here, so the number gating a cost
/// sits beside the cost and can be argued with by editing one line.
const TOLERANCES: [(&str, f64); 2] = [
    // Fifteen bound unix sockets and fifteen threads parked in accept per iteration, at around
    // 90 µs/pane - three orders of magnitude above everything else here, and the only
    // benchmark whose time is mostly the kernel's. Measured spread 1.42x, so 2.00x would have
    // left it barely a margin at all.
    ("pane.channel", 3.0),
    // Builds libghostty's key and mouse encoders per pane, which crosses into a dylib and reads
    // its mode tables. Cheaper than pane.channel and steadier, but not arithmetic either.
    ("pane.encoder", 2.0),
];

/// What everything else gets. Comfortably outside the 1.14x these were measured to vary by,
/// and tight enough that a cost growing by half is something somebody has to look at.
const DEFAULT_TOLERANCE: f64 = 1.5;

fn with_tolerance(mut cost: Cost) -> Cost {
    let found = TOLERANCES.iter().find(|(name, _)| *name == cost.name);
    cost.tolerance = Some(found.map_or(DEFAULT_TOLERANCE, |(_, tolerance)| *tolerance));
    cost
}

fn main() {
    let options = Options::parse();
    let costs = measure_everything();

    println!("{}", table(&costs));
    println!();
    println!("{}", pending(&PENDING));
    println!();

    if options.recording {
        record(&costs, &options);
    }
    judge(&costs, &options);
}

struct Options {
    recording: bool,
    baseline: String,
    tolerance: f64,
    /// What ./dev found the machine doing. Absent when the harness was run by hand, which is
    /// worth distinguishing from a quiet machine rather than defaulting to zero.
    load: Option<Load>,
}

impl Options {
    fn parse() -> Options {
        let mut options = Options {
            recording: false,
            baseline: "perf/baseline.json".to_string(),
            tolerance: 2.0,
            load: None,
        };
        let mut averages: Option<[f64; 3]> = None;
        let mut counts: Option<[usize; 2]> = None;
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let mut value = |name: &str| {
                arguments.next().unwrap_or_else(|| {
                    eprintln!("muster-perf: {name} needs a value");
                    std::process::exit(2);
                })
            };
            match argument.as_str() {
                "--record" => options.recording = true,
                "--baseline" => options.baseline = value("--baseline"),
                "--tolerance" => {
                    options.tolerance = value("--tolerance").parse().unwrap_or(options.tolerance);
                }
                "--load" => averages = triple(&value("--load")),
                "--cores" => counts = pair(&value("--cores")),
                "-h" | "--help" => {
                    println!("{USAGE}");
                    std::process::exit(0);
                }
                other => {
                    eprintln!("muster-perf: unknown argument {other}\n{USAGE}");
                    std::process::exit(2);
                }
            }
        }
        // Both halves or neither. A load average without a core count beside it says nothing -
        // four is quiet on a laptop and desperate on a Raspberry Pi - so a run that was handed
        // one and not the other records that it was not told, rather than half a fact.
        if let (Some([one, five, fifteen]), Some([cores, fast_cores])) = (averages, counts) {
            options.load = Some(Load { one, five, fifteen, cores, fast_cores });
        }
        options
    }
}

/// Three comma-separated numbers, or nothing if that is not what arrived.
fn triple(text: &str) -> Option<[f64; 3]> {
    let parsed: Vec<f64> = text.split(',').filter_map(|field| field.trim().parse().ok()).collect();
    match parsed[..] {
        [one, five, fifteen] => Some([one, five, fifteen]),
        _ => None,
    }
}

/// Two comma-separated counts, or nothing.
fn pair(text: &str) -> Option<[usize; 2]> {
    let parsed: Vec<usize> =
        text.split(',').filter_map(|field| field.trim().parse().ok()).collect();
    match parsed[..] {
        [first, second] => Some([first, second]),
        _ => None,
    }
}

/// What one structural change costs a window that is full.
///
/// Every pane event republishes the whole view - that is what keeps the shell from holding a
/// picture it has to patch - so this runs whenever anything moves, and it is the one cost
/// that scales with how many panes are on screen rather than with how much they print.
///
/// Per pane rather than per view, so the number stays comparable as the budgeted window size
/// changes, and so that a build which made it quadratic shows up as one that no longer
/// matches the one-pane case.
fn view_cost() -> Cost {
    let (composition, mirror) = full_window(BUDGETED_PANES);
    let daemon = DaemonId::new("local");
    measure("view.build", "ns/pane", BUDGETED_PANES * 200, 20, 5, || {
        for _ in 0..200 {
            // The daemon answers local here: what this measures is building a view, not
            // reaching a machine. The pane closure answers without a lookup for the same
            // reason - a registry behind a lock would put its cost in this number. Text size
            // answers the configured one, which is what every pane in a window nobody has
            // sized reports, and so does the bridge count: a window measured mid-recovery is
            // not the window this budget is about.
            let view = View::of(
                &composition,
                |named| (named == &daemon).then_some(&mirror),
                |_| Some("/tmp/muster-daemon.sock".to_string()),
                |_| false,
                |_, pane| ViewPane {
                    id: pane.clone(),
                    link_socket_path: Some(pane.to_string()),
                    font_size_offset: 0,
                    bridge_restarts: 0,
                },
            );
            black_box(view.regions.len());
        }
    })
}

/// What one changed title costs, which is the budget that decides whether a second line is
/// affordable at all.
///
/// A harness rewrites its terminal title as it works, and every such change republishes the
/// whole roster - so this lands at frequency times cardinality, which is the shape this file
/// exists to hold. The frequency half is the daemon's: muster-daemon announces a title only
/// when it differs from the one it holds, so a program repeating its title costs nothing here,
/// and one rotating a spinner through it costs this once per frame. What is left is the
/// per-change cost, and this is it.
///
/// Per pane rather than per roster, like `view.build` beside it, so the number stays comparable
/// as the budgeted window size changes and a build that made it quadratic stops matching.
fn roster_cost() -> Cost {
    let (composition, mirror) = full_window(BUDGETED_PANES);
    let daemon = DaemonId::new("local");
    let showing = std::collections::BTreeSet::new();
    measure("roster.build", "ns/pane", BUDGETED_PANES * 200, 20, 5, || {
        for _ in 0..200 {
            let roster =
                Roster::of(&composition, |named| (named == &daemon).then_some(&mirror), &showing);
            black_box(roster.tabs.len());
        }
    })
}

/// What agent detection pays to read a pane's screen, which it does for every pane every
/// few hundred milliseconds. The same 24 rows two ways: the formatter in one call, and the
/// per-cell read tests use, which is what detection would cost without the formatter.
fn screen_reads() -> Vec<Cost> {
    let mut costs = Vec::new();
    if let Ok(mut terminal) = Terminal::new(80, 24) {
        let screen: Vec<u8> = (0..24)
            .flat_map(|row| format!("\r\n{row:>3} {}", "agent output ".repeat(5)).into_bytes())
            .collect();
        terminal.write(&screen);
        costs.push(measure("vt.text_read", "ns/row", 24, 200, 20, || {
            black_box(terminal.text(0, 23).len());
        }));
        costs.push(measure("vt.cell_read", "ns/row", 24, 20, 5, || {
            black_box(terminal.viewport(80, 24).rows.len());
        }));
    }
    costs
}

/// What attaching to a pane costs per row of its history: composing the replay in the
/// daemon, and parsing it in the surface. MIP-3 section 13's attach target is set from
/// these; ten thousand rows is the history that target names.
fn replay_costs() -> Vec<Cost> {
    const HISTORY: usize = 10_000;
    let mut costs = Vec::new();
    if let Ok(mut terminal) = Terminal::with_options(muster_vt::TerminalOptions {
        scrollback_bytes: Some(usize::MAX),
        ..muster_vt::TerminalOptions::new(80, 24)
    }) {
        let history: Vec<u8> = (0..HISTORY)
            .flat_map(|row| {
                format!("{row:>6} \x1b[32magent\x1b[0m output {}\r\n", "x".repeat(40)).into_bytes()
            })
            .collect();
        terminal.write(&history);
        let replay = terminal.replay();
        costs.push(measure("vt.replay_compose", "ns/row", HISTORY, 10, 2, || {
            black_box(terminal.replay().len());
        }));
        costs.push(measure("vt.replay_parse", "ns/row", HISTORY, 10, 2, || {
            if let Ok(mut surface) = Terminal::with_options(muster_vt::TerminalOptions {
                scrollback_bytes: Some(usize::MAX),
                ..muster_vt::TerminalOptions::new(80, 24)
            }) {
                surface.write(&replay);
            }
        }));
    }
    costs
}

fn measure_everything() -> Vec<Cost> {
    let output = agent_output();
    let mut costs = vec![measure("output.vt_parse", "ns/byte", output.len(), 20, 5, || {
        // A fresh terminal per iteration: replaying a repaint into a terminal that already
        // holds it measures a different, cheaper thing.
        let Ok(mut terminal) = Terminal::new(80, 24) else { return };
        terminal.write(&output);
    })];

    costs.extend(screen_reads());
    costs.extend(replay_costs());

    // What every keystroke pays in the core before it leaves for the daemon: the keymap,
    // option-as-alt, and building the event. The daemon encodes it against the pane's modes,
    // so this is the whole of the core's share, and worth a standing number because this is
    // the one path where a regression is felt rather than measured.
    //
    // Into a sink that drops everything, because what a real sink does is queue the event
    // for a socket, which is the daemon connection's cost rather than the core's.
    let input = PaneInput::new(
        PaneId::new("p1"),
        Arc::new(Discarded) as Arc<dyn InputSink>,
        &PaneInputSettings::default(),
    );
    let keystrokes = [
        typed(Key::KeyH, Modifiers::NONE, "h"),
        typed(Key::KeyC, Modifiers::CONTROL, "c"),
        typed(Key::Enter, Modifiers::SHIFT, "\r"),
        typed(Key::Backspace, Modifiers::SUPER, "\u{7f}"),
    ];
    costs.push(measure("input.route", "ns/key", keystrokes.len() * 100, 100, 5, || {
        for _ in 0..100 {
            for key in &keystrokes {
                input.send(key);
            }
        }
    }));

    // The control plane's per-event cost. Measurable at all because the mirror has no I/O in
    // it: this is the same fold a live subscription runs, with the socket left out.
    let (_, mut mirror) = full_window(BUDGETED_PANES);
    let events = window_at_work(&mirror);
    costs.push(measure("mirror.apply", "ns/event", events.len() * 20, 20, 5, || {
        // Into one mirror across iterations, which is only honest because the stream leaves
        // the window as it found it: every event changes something on every pass, as a real
        // one does.
        //
        // Cloned per apply, because the fold takes its event by value and a live one arrives
        // freshly decoded; copying a whole pane record is part of what an event costs.
        for _ in 0..20 {
            for event in &events {
                black_box(mirror.apply(event.clone()).len());
            }
        }
    }));

    // What the shell/core boundary costs per keystroke: encode a request, decode it, answer,
    // encode the answer. MIP-1 argued this seam can afford protobuf because it carries
    // events at human rates rather than bytes - around ten keystrokes a second - and this is
    // the number that claim is checkable against.
    //
    // No pane is attached, so what is measured is the crossing itself rather than the
    // routing behind it, which `input.route` already covers separately.
    let request = key_down_request();
    costs.push(measure("seam.dispatch", "ns/event", 100, 200, 5, || {
        for _ in 0..100 {
            black_box(muster::dispatch(&request).len());
        }
    }));

    costs.push(view_cost());
    costs.push(roster_cost());

    // What a full window holds open per pane, which is the half of "fast is a feature" that is
    // fixed cost rather than throughput. None of it is visible in a per-byte number, because
    // none of it happens per byte.
    //
    // Two numbers rather than one, because they are paid in different processes and regress
    // for unrelated reasons. The link socket and its thread are the window's; the encoders are
    // the daemon's, and are libghostty-vt's cost that a replacement would have to match.
    costs.push(measure("pane.channel", "ns/pane", BUDGETED_PANES, 10, 3, || {
        let held: Vec<_> = (0..BUDGETED_PANES).filter_map(open_channel).collect();
        black_box(held.len());
    }));
    // Twenty windows' worth per iteration rather than one, so that the fastest sample is tens
    // of microseconds instead of one or two, which is too close to the clock's resolution to
    // judge. Still ns/pane, so the figure means what it always meant.
    let encoders = BUDGETED_PANES * 20;
    costs.push(measure("pane.encoder", "ns/pane", encoders, 10, 3, || {
        for _ in 0..encoders {
            black_box(KeyEncoder::new(KeyModes::default()).is_ok());
            black_box(MouseEncoder::new().is_ok());
        }
    }));

    costs
}

/// What an agent's pane prints while it works.
///
/// Every such byte is parsed twice, by the daemon's terminal for the pane and by the surface
/// drawing it (MIP-3, section 4), and this is the cost of one of those.
///
/// Built here rather than recorded, because the recordings in the corpus are herdr's repaints
/// of a screen, which is not what muster-daemon relays: it sends a program's own bytes. So
/// this is those bytes, shaped the way a coding agent writes them. Mostly styled lines of
/// text, some of them drawn in box characters, and after every few lines a status block redrawn
/// in place - cursor up, erase, a spinner frame and a line of progress - inside a synchronized
/// update, which is how the harnesses Muster runs repaint without tearing. A stream of plain
/// ASCII would measure the parser's fastest path and nothing an agent actually sends.
fn agent_output() -> Vec<u8> {
    const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut output = Vec::new();
    for turn in 0..400 {
        output.extend_from_slice(
            format!(
                "\x1b[1m\x1b[38;2;215;119;87m⏺\x1b[0m Reading \x1b[1msrc/turn_{turn}.rs\x1b[0m\r\n"
            )
            .as_bytes(),
        );
        output.extend_from_slice("  \x1b[2m⎿\x1b[0m  Read 120 lines\r\n".as_bytes());
        output
            .extend_from_slice(format!("\x1b[38;5;244m╭{}╮\x1b[0m\r\n", "─".repeat(76)).as_bytes());
        for line in 0..4 {
            output.extend_from_slice(
                format!(
                    "\x1b[38;5;244m│\x1b[0m \x1b[32m+\x1b[0m    let value_{line} = \
                     \x1b[33mcompute\x1b[0m(\x1b[36m{turn}\x1b[0m, &mut state);{:>30}\
                     \x1b[38;5;244m│\x1b[0m\r\n",
                    ""
                )
                .as_bytes(),
            );
        }
        output
            .extend_from_slice(format!("\x1b[38;5;244m╰{}╯\x1b[0m\r\n", "─".repeat(76)).as_bytes());
        for frame in 0..3 {
            output.extend_from_slice(
                format!(
                    "\x1b[?2026h\x1b[2A\r\x1b[J\x1b[38;2;215;119;87m{}\x1b[0m \
                     Thinking… \x1b[2m({}s · ↓ {} tokens · esc to interrupt)\x1b[0m\r\n\
                     \x1b[2m  ? for shortcuts\x1b[0m\r\n\x1b[?2026l",
                    SPINNER[(turn * 3 + frame) % SPINNER.len()],
                    turn / 4,
                    turn * 37 + frame
                )
                .as_bytes(),
            );
        }
    }
    output
}

/// A window as full as Muster budgets for: one region, one tab, `panes` panes in a tree.
///
/// Nested down one side rather than balanced, because that is what splitting the pane you
/// are looking at produces over and over, and it is the deepest tree the same pane count can
/// make - so a walk that is worse than linear in depth shows up here rather than hiding
/// behind a shape nobody builds by hand.
fn full_window(panes: usize) -> (Composition, Mirror) {
    let tab = TabId::new("t1");
    let ids: Vec<PaneId> = (0..panes).map(|index| PaneId::new(format!("p{index}"))).collect();

    let mut mirror = Mirror::new();
    mirror.bootstrap(Snapshot {
        seq: 1,
        instance: 1,
        tabs: vec![Tab {
            id: tab.clone(),
            label: Some("t1".to_string()),
            generation: 1,
            root: nested(&ids),
            zoomed: None,
        }],
        panes: ids.iter().map(|id| agent_pane(id, &tab)).collect(),
        restored_from_file: false,
        restoring: false,
        human: std::collections::BTreeMap::new(),
    });

    let daemon = DaemonId::new("local");
    let mut composition = Composition::new();
    composition.attach_daemon(Daemon {
        id: daemon.clone(),
        endpoint: Endpoint::Local { socket_path: Some("/tmp/muster-perf.sock".to_string()) },
    });
    composition.open_region(&daemon, tab);
    composition.reconcile(&daemon, &mirror);
    (composition, mirror)
}

/// Each pane split off the one before it, down one side.
fn nested(ids: &[PaneId]) -> LayoutNode {
    let (last, rest) = ids.split_last().expect("a window has at least one pane");
    let mut root = LayoutNode::Pane(last.clone());
    for id in rest.iter().rev() {
        root = LayoutNode::Split {
            axis: SplitAxis::Columns,
            ratio: 0.5,
            first: Box::new(LayoutNode::Pane(id.clone())),
            second: Box::new(root),
        };
    }
    root
}

/// One pane of the window this is budgeted for: a harness in it, and a title on it.
///
/// Also the expensive shape: the roster decides per row whether a title says anything the
/// label does not, and a pane with neither would skip that work. Facts too, because an agent
/// reporting itself is the common case and they travel in every record.
fn agent_pane(id: &PaneId, tab: &TabId) -> Pane {
    Pane {
        id: id.clone(),
        tab: tab.clone(),
        agent_state: AgentState::Idle,
        finished_unseen: false,
        agent: Some("claude".to_string()),
        compactable: true,
        cwd: "/tmp".to_string(),
        name: None,
        title: Some("first working build".to_string()),
        command: Some("claude".to_string()),
        facts: AgentFacts {
            context_used: Some(12.0),
            subagents: 0,
            model: Some("opus".to_string()),
            cost_usd: Some(0.42),
            other: std::collections::BTreeMap::new(),
            waiting: None,
        },
        reported: true,
        unreadable: false,
        adapter: muster_core::mirror::Adapter::Reporting,
    }
}

/// What a daemon says while a full window of agents works, in the mix it says it.
///
/// Built from the mirror's own panes, and ends where it began, so it can be applied over and
/// over and change something every time. Per pane: it starts working, retitles itself, reports
/// how much context it has used, and goes idle again - the agent-state and fact records that
/// make up nearly everything a busy window hears. Once per pass, a pane is split off and closed
/// again, which is the structural change: an opened pane, a tree naming it, the pane closed,
/// the tree without it.
fn window_at_work(mirror: &Mirror) -> Vec<BackendEvent> {
    let panes: Vec<Pane> = mirror.panes().cloned().collect();
    let tabs: Vec<Tab> = mirror.tabs().cloned().collect();
    let mut events = Vec::new();
    for pane in &panes {
        let mut working = pane.clone();
        working.agent_state = AgentState::Working;
        events.push(BackendEvent::PaneChanged(working.clone()));
        working.title = Some("running the suite".to_string());
        events.push(BackendEvent::PaneChanged(working.clone()));
        working.facts.context_used = Some(31.0);
        working.facts.cost_usd = Some(0.97);
        events.push(BackendEvent::PaneChanged(working));
    }
    if let (Some(tab), Some(beside)) = (tabs.first(), panes.last()) {
        let extra = PaneId::new("p-extra");
        let mut opened = beside.clone();
        opened.id = extra.clone();
        let mut split = tab.clone();
        split.root = LayoutNode::Split {
            axis: SplitAxis::Rows,
            ratio: 0.5,
            first: Box::new(tab.root.clone()),
            second: Box::new(LayoutNode::Pane(extra.clone())),
        };
        events.push(BackendEvent::PaneOpened(opened));
        events.push(BackendEvent::TabChanged(split));
        events.push(BackendEvent::PaneClosed(extra));
        events.push(BackendEvent::TabChanged(tab.clone()));
    }
    events.extend(panes.into_iter().map(BackendEvent::PaneChanged));
    events
}

/// What the core opens for each pane it shows: the socket that pane's bridge reports on, a
/// thread parked in `accept` on it, and the input path.
///
/// A stand-in for the seam's `PaneLink`, which is its own and out of reach from here. What it
/// keeps is the part with a cost - the bind and the thread, which are the kernel's work - and
/// what it drops is the reporting a link does once a bridge dials, which never happens here.
/// `PaneInput::new` is the real one.
fn open_channel(index: usize) -> Option<(Link, PaneInput)> {
    let path = std::env::temp_dir()
        .join(format!("muster-perf-{}-{index}.sock", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).ok()?;
    let closing = Arc::new(AtomicBool::new(false));
    let told = Arc::clone(&closing);
    std::thread::spawn(move || {
        while let Ok((_stream, _)) = listener.accept() {
            if told.load(Ordering::Acquire) {
                return;
            }
        }
    });
    let input = PaneInput::new(
        PaneId::new(format!("p{index}")),
        Arc::new(Discarded) as Arc<dyn InputSink>,
        &PaneInputSettings::default(),
    );
    Some((Link { path, closing }, input))
}

/// A bound link socket, closed the way the seam closes one.
struct Link {
    path: String,
    closing: Arc<AtomicBool>,
}

impl Drop for Link {
    fn drop(&mut self) {
        // Knock, then take the door away: nothing else wakes a thread parked in `accept`.
        self.closing.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A daemon connection that takes every event and sends none of them.
#[derive(Debug)]
struct Discarded;

impl InputSink for Discarded {
    fn send(&self, _pane: &PaneId, event: InputEvent) -> Result<(), NotSent> {
        black_box(event);
        Ok(())
    }

    fn description(&self) -> &'static str {
        "nowhere"
    }
}

/// One press, encoded the way the shell encodes one.
fn key_down_request() -> Vec<u8> {
    let mut key = muster::proto::KeyEvent {
        action: "press".to_string(),
        key: "KeyH".to_string(),
        text: "h".to_string(),
        ..muster::proto::KeyEvent::default()
    };
    key.modifiers.push("control".to_string());
    muster::proto::Request::new(muster::proto::request::Payload::KeyDown(muster::proto::KeyDown {
        key: Some(key),
        ..muster::proto::KeyDown::default()
    }))
    .encode_to_vec()
}

fn typed(key: Key, modifiers: Modifiers, text: &str) -> KeyEvent {
    KeyEvent { key, modifiers, text: text.to_string(), ..KeyEvent::default() }
}

fn record(costs: &[Cost], options: &Options) -> ! {
    let path = &options.baseline;
    let baseline = Baseline {
        recorded: muster_core::diagnostics::format_iso8601(
            muster_core::diagnostics::wall_clock_millis(),
        ),
        machine: machine_description(),
        load: options.load,
        costs: costs.iter().cloned().map(with_tolerance).collect(),
    };
    let Ok(mut json) = serde_json::to_string_pretty(&baseline) else {
        eprintln!("muster-perf: could not encode the baseline");
        std::process::exit(1);
    };
    json.push('\n');

    if let Some(parent) = Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(path, json) {
        eprintln!("muster-perf: {path}: {error}");
        std::process::exit(1);
    }
    println!("recorded {} costs to {path}", costs.len());
    std::process::exit(0);
}

fn judge(costs: &[Cost], options: &Options) -> ! {
    let baseline = std::fs::read_to_string(&options.baseline)
        .ok()
        .and_then(|text| serde_json::from_str::<Baseline>(&text).ok());
    let Some(baseline) = baseline else {
        eprint!(
            "muster-perf: no baseline at {}, so nothing gates these numbers.\n\
             Record one with `muster-perf --record` once the machine is quiet.\n\n",
            options.baseline
        );
        std::process::exit(2);
    };

    let notes = context(&baseline, &machine_description(), options.load);
    if !notes.is_empty() {
        println!("{notes}");
    }

    let comparison = compare(costs, &baseline, options.tolerance);
    println!("{}", verdict(&comparison));
    std::process::exit(i32::from(!comparison.is_clean()));
}

fn machine_description() -> String {
    // SAFETY: uname writes into a utsname we own and reads nothing else.
    let mut info: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    if unsafe { libc::uname(&raw mut info) } != 0 {
        return "unknown".to_string();
    }
    let read = |field: &[libc::c_char]| {
        field
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| byte.cast_unsigned() as char)
            .collect::<String>()
    };
    format!("{}-{} {}", read(&info.machine), read(&info.sysname), read(&info.release))
}
