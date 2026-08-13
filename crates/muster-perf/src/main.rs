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

use muster_core::input::{Key, KeyEvent, Keymap, Modifiers, Resolution, TerminalModeProfile};
use muster_core::mirror::{BackendEvent, Mirror};
use muster_herdr::{EventDecoder, FrameDecoder, PaneFrame, PaneStreamEvent};
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

/// The desiderata name four budgets - per byte, per event, per render - at 1 and 15 panes.
/// One of them has no code to measure yet. It is printed rather than omitted, because a
/// budget nobody wrote down reads the same as a budget nobody exceeded.
const PENDING: [(&str, &str); 1] = [(
    "render at 15 panes",
    "per-byte cost is linear and already covered; what 15 panes actually tests is \
     aggregate scheduling across surfaces, which needs splits. Kan a_26BJGL7VZ.",
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

    costs
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
