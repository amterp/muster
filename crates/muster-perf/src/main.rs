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
use std::path::{Path, PathBuf};
use std::sync::Arc;

use muster_core::AgentState;
use muster_core::composition::{Composition, Daemon, DaemonId, Endpoint, View};
use muster_core::input::{
    Key, KeyEvent, Keymap, Modifiers, PaneInput, PaneInputSettings, Resolution, TerminalModeProfile,
};
use muster_core::mirror::backend::{
    Focus, Layout, LayoutNode, Pane, PaneId, Snapshot, SplitAxis, Tab, TabId, Workspace,
    WorkspaceId,
};
use muster_core::mirror::{BackendEvent, Mirror};
use muster_herdr::{EventDecoder, FrameDecoder, PaneControlChannel, PaneFrame, PaneStreamEvent};
use muster_perf::{Baseline, Cost, compare, measure, pending, table, verdict};
use muster_vt::{KeyEncoder, Terminal};
use prost::Message;

const USAGE: &str = "\
usage: muster-perf [--record] [--baseline <path>] [--corpus <path>] [--tolerance <x>]

Measures the per-unit costs on Muster's hot paths and compares them to a baseline.

  --record       write this run's numbers as the new baseline instead of judging
  --baseline     where the baseline lives (default perf/baseline.json)
  --corpus       recorded frames to replay (default corpus)
  --tolerance    how many times the recorded cost still passes (default 2.0)";

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

fn main() {
    let options = Options::parse();
    let streams = recorded_frame_streams(&options.corpus);
    if streams.is_empty() {
        eprint!(
            "muster-perf: no recorded frames under {}.\n\
             The per-byte budgets replay real daemon output, so without the corpus this run \
             would report nothing while exiting 0 - which reads as a pass. Run from the repo \
             root, or pass --corpus.\n\n",
            options.corpus
        );
        std::process::exit(2);
    }

    let costs = measure_everything(&streams);

    println!("{}", table(&costs));
    println!();
    println!("{}", pending(&PENDING));
    println!();

    if options.recording {
        record(&costs, &options.baseline);
    }
    judge(&costs, &options);
}

struct Options {
    recording: bool,
    baseline: String,
    corpus: String,
    tolerance: f64,
}

impl Options {
    fn parse() -> Options {
        let mut options = Options {
            recording: false,
            baseline: "perf/baseline.json".to_string(),
            corpus: "corpus".to_string(),
            tolerance: 2.0,
        };
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
                "--corpus" => options.corpus = value("--corpus"),
                "--tolerance" => {
                    options.tolerance = value("--tolerance").parse().unwrap_or(options.tolerance);
                }
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
        options
    }
}

/// Every recorded frame stream, concatenated.
///
/// Recorded from a real daemon rather than generated, so the shape being timed is the shape
/// that actually arrives: one 35 KB attach repaint followed by small diffs. A synthetic
/// stream of uniform frames would measure a workload Muster never sees.
fn recorded_frame_streams(corpus: &str) -> Vec<Vec<u8>> {
    let mut found: Vec<PathBuf> = Vec::new();
    let Ok(versions) = std::fs::read_dir(Path::new(corpus)) else { return Vec::new() };
    for version in versions.flatten() {
        let Ok(captures) = std::fs::read_dir(version.path()) else { continue };
        for capture in captures.flatten() {
            let stream = capture.path().join("frames.ndjson");
            if stream.is_file() {
                found.push(stream);
            }
        }
    }
    found.sort();
    found.iter().filter_map(|path| std::fs::read(path).ok()).collect()
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
            // The last closure is the daemon's transport, and it is local here: what this
            // measures is building a view, not reaching a machine.
            let view = View::of(
                &composition,
                |named| (named == &daemon).then_some(&mirror),
                |_, pane| Some(pane.to_string()),
                |_| None,
                |_| Some("/tmp/herdr.sock".to_string()),
            );
            black_box(view.regions.len());
        }
    })
}

fn measure_everything(streams: &[Vec<u8>]) -> Vec<Cost> {
    let wire_bytes: usize = streams.iter().map(Vec::len).sum();
    let frames: Vec<PaneFrame> = streams
        .iter()
        .flat_map(|stream| {
            let mut decoder = FrameDecoder::new();
            decoder.consume(stream).into_iter().filter_map(|event| match event {
                PaneStreamEvent::Frame(frame) => Some(frame),
                PaneStreamEvent::Closed { .. } => None,
            })
        })
        .collect();
    let ansi_bytes: usize = frames.iter().map(|frame| frame.bytes.len()).sum();

    // Two separate budgets that a single "frame cost" would hide. Decoding scales with the
    // wire bytes the bridge reads; VT parsing scales with the ANSI those unwrap to, which is
    // smaller and far more expensive per byte.
    let mut costs = vec![
        measure("frame.decode", "ns/byte", wire_bytes, 50, 5, || {
            for stream in streams {
                let mut decoder = FrameDecoder::new();
                black_box(decoder.consume(stream).len());
            }
        }),
        measure("frame.vt_parse", "ns/byte", ansi_bytes.max(1), 20, 5, || {
            // A fresh terminal per iteration: replaying a repaint into a terminal that
            // already holds it measures a different, cheaper thing.
            let Ok(terminal) = Terminal::new(80, 24) else { return };
            for frame in &frames {
                terminal.write(&frame.bytes);
            }
        }),
    ];

    // What every keystroke pays before it reaches a socket. Small by construction - the
    // encoder is built once and holds the pane's modes - and worth a standing number because
    // this is the one path where a regression is felt rather than measured.
    match KeyEncoder::new(TerminalModeProfile::UNKNOWN_PANE) {
        Err(error) => {
            eprintln!(
                "muster-perf: skipping input.encode - the key encoder would not build: {error}"
            );
        }
        Ok(encoder) => {
            let keymap = Keymap::default();
            let keystrokes = [
                typed(Key::KeyH, Modifiers::NONE, "h"),
                typed(Key::KeyC, Modifiers::CONTROL, "c"),
                typed(Key::Enter, Modifiers::SHIFT, "\r"),
                typed(Key::Backspace, Modifiers::SUPER, "\u{7f}"),
            ];
            costs.push(measure("input.encode", "ns/key", keystrokes.len() * 100, 100, 5, || {
                for _ in 0..100 {
                    for key in &keystrokes {
                        match keymap.resolve(key) {
                            Resolution::Text(bytes) => black_box(bytes.len()),
                            Resolution::Unbound => {
                                black_box(encoder.encode(key).unwrap_or_default().len())
                            }
                            Resolution::ServerEncoded(_) | Resolution::Action(_) => 0,
                        };
                    }
                }
            }));
        }
    }

    // The control plane's per-event cost, which is the half of "fast is a feature" that had
    // never had a number. Measurable at all because the mirror has no I/O in it: this is the
    // same fold a live subscription runs, with the socket left out.
    //
    // The stream is the recorded lifecycle capture rather than one shape repeated, so what
    // is timed is the mix a real session produces - mostly upserts that change nothing,
    // because the daemon replays and re-announces far more often than it changes anything.
    let events =
        recorded_backend_events(&PathBuf::from("corpus/herdr-0.8.0/lifecycle/events.ndjson"));
    if events.is_empty() {
        eprintln!(
            "muster-perf: skipping mirror.apply - no recorded events under \
             corpus/herdr-0.8.0/lifecycle/"
        );
    } else {
        costs.push(measure("mirror.apply", "ns/event", events.len() * 20, 20, 5, || {
            // A fresh mirror per iteration, because applying into one that already holds
            // the session measures convergence rather than the first build, and the two
            // differ by every insert.
            let mut mirror = Mirror::new();
            for _ in 0..20 {
                for event in &events {
                    black_box(mirror.apply(event.clone()).len());
                }
            }
        }));
    }

    // What the shell/core boundary costs per keystroke: encode a request, decode it, answer,
    // encode the answer. MIP-1 argued this seam can afford protobuf because it carries
    // events at human rates rather than bytes - around ten keystrokes a second - and this is
    // the number that claim is checkable against.
    //
    // No pane is attached, so what is measured is the crossing itself rather than the
    // encoder behind it, which `input.encode` already covers separately.
    let request = key_down_request();
    costs.push(measure("seam.dispatch", "ns/event", 100, 200, 5, || {
        for _ in 0..100 {
            black_box(muster::dispatch(&request).len());
        }
    }));

    costs.push(view_cost());

    // What Muster holds open per pane, which is the half of "fast is a feature" that is fixed
    // cost rather than throughput: a full window is fifteen bound sockets, fifteen threads
    // waiting on them, and fifteen key encoders. None of it is visible in a per-byte number,
    // because none of it happens per byte.
    //
    // Two numbers rather than one, because they regress for unrelated reasons and a single
    // one would not say which. The socket and its thread are Muster's own; the encoder is
    // libghostty-vt's, and is the cost a replacement renderer would have to match.
    costs.push(measure("pane.channel", "ns/pane", BUDGETED_PANES, 10, 3, || {
        let held: Vec<_> = (0..BUDGETED_PANES).filter_map(bind_pane_socket).collect();
        black_box(held.len());
    }));
    costs.push(measure("pane.encoder", "ns/pane", BUDGETED_PANES, 10, 3, || {
        for _ in 0..BUDGETED_PANES {
            black_box(KeyEncoder::new(TerminalModeProfile::UNKNOWN_PANE).is_ok());
        }
    }));

    costs
}

/// A window as full as Muster budgets for: one region, one tab, `panes` panes in a tree.
///
/// Nested down one side rather than balanced, because that is what splitting the pane you
/// are looking at produces over and over, and it is the deepest tree the same pane count can
/// make - so a walk that is worse than linear in depth shows up here rather than hiding
/// behind a shape nobody builds by hand.
fn full_window(panes: usize) -> (Composition, Mirror) {
    let (workspace, tab) = (WorkspaceId::new("w1"), TabId::new("w1:t1"));
    let ids: Vec<PaneId> = (0..panes).map(|index| PaneId::new(format!("w1:p{index}"))).collect();

    let mut root = LayoutNode::Pane(ids[panes - 1].clone());
    for id in ids.iter().rev().skip(1) {
        root = LayoutNode::Split {
            axis: SplitAxis::Columns,
            ratio: 0.5,
            first: Box::new(LayoutNode::Pane(id.clone())),
            second: Box::new(root),
        };
    }

    let mut mirror = Mirror::new();
    mirror.bootstrap(Snapshot {
        workspaces: vec![Workspace { id: workspace.clone(), label: "w1".to_string() }],
        tabs: vec![Tab { id: tab.clone(), workspace: workspace.clone(), label: "t1".to_string() }],
        panes: ids
            .iter()
            .map(|id| Pane {
                id: id.clone(),
                tab: tab.clone(),
                workspace: workspace.clone(),
                agent_state: AgentState::Idle,
                agent: None,
                cwd: "/tmp".to_string(),
                name: None,
                title: None,
            })
            .collect(),
        layouts: vec![Layout { tab: tab.clone(), root, focused: None, zoomed: None }],
        focus: Focus::default(),
        agent_state_seq: None,
    });

    let daemon = DaemonId::new("local");
    let mut composition = Composition::new();
    composition.attach_daemon(Daemon {
        id: daemon.clone(),
        endpoint: Endpoint::Local { socket_path: Some("/tmp/herdr-perf.sock".to_string()) },
    });
    composition.open_region(&daemon, workspace, tab);
    composition.reconcile(&daemon, &mirror);
    (composition, mirror)
}

/// One pane's socket, bound and listening, with the input path built over it.
///
/// The daemon's own side is deliberately absent. What a herdr connection costs is herdr's
/// number and belongs in an observation, not in a budget that gates this repo's builds.
fn bind_pane_socket(index: usize) -> Option<(Arc<PaneControlChannel>, PaneInput)> {
    let path = std::env::temp_dir()
        .join(format!("muster-perf-{}-{index}.sock", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let control = Arc::new(PaneControlChannel::bind(path, || {}).ok()?);
    let encoder = Arc::new(KeyEncoder::new(TerminalModeProfile::UNKNOWN_PANE).ok()?);
    let input = PaneInput::new(
        Arc::clone(&control) as Arc<_>,
        None,
        encoder,
        &PaneInputSettings::default(),
    );
    Some((control, input))
}

/// Every event in a recorded subscription, already translated.
///
/// Decoded once, outside the timing loop: this budget is the mirror's fold, and the
/// decoder that feeds it is a separate cost that would otherwise be folded in silently.
fn recorded_backend_events(path: &Path) -> Vec<BackendEvent> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    EventDecoder::new().consume(&bytes)
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
    muster::proto::Request {
        payload: Some(muster::proto::request::Payload::KeyDown(muster::proto::KeyDown {
            key: Some(key),
            ..muster::proto::KeyDown::default()
        })),
    }
    .encode_to_vec()
}

fn typed(key: Key, modifiers: Modifiers, text: &str) -> KeyEvent {
    KeyEvent { key, modifiers, text: text.to_string(), ..KeyEvent::default() }
}

fn record(costs: &[Cost], path: &str) -> ! {
    let baseline = Baseline {
        recorded: muster_core::diagnostics::format_iso8601(
            muster_core::diagnostics::wall_clock_millis(),
        ),
        machine: machine_description(),
        costs: costs.to_vec(),
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

    if baseline.machine != machine_description() {
        println!(
            "note: baseline recorded on {}, running on {}. A cross-machine comparison \
             explains a failure that is not a regression.",
            baseline.machine,
            machine_description()
        );
    }

    let comparison = compare(costs, &baseline, options.tolerance);
    println!("{}", verdict(&comparison, options.tolerance));
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
