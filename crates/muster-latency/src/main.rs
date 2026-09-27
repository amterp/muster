//! muster-latency: input-to-glyph through muster-daemon, beside the bare PTY it is judged
//! against (MIP-3, section 13).
//!
//! These rows are what the cut-over is judged by. A keystroke goes in as the app will send it,
//! a key event on the daemon's input connection, and the glyph is the letter coming back: on
//! the pane's stream read directly, which is the daemon's share, and on the PTY the real bridge
//! writes to, where the surface would parse it. The surface's parse and the GPU are in no
//! number here.
//!
//! Every pane runs `cat` in canonical mode, so the echo is the terminal's own, as in the
//! herdr measurements it replaced.

mod daemon;
mod glyph;
mod stats;
mod surface;
mod throughput;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daemon::{Daemon, Stream};
use stats::{Row, Verdict};
use surface::{Pace, Surface};

const USAGE: &str = "\
usage: muster-latency (--daemon <path> | --socket <path>) --bridge <path>
                      [--samples <n>] [--panes <n>] [--flood-lines <n>]
                      [--flood-surface slow|fast] [--json]
       muster-latency --throughput --daemon <path> [--bytes <n>] [--runs <n>]

Times input-to-glyph through muster-daemon beside the bare PTY: the pane's stream read
directly, and the real bridge (muster-bridge --daemon-socket) onto a PTY. Idle, in a
window of --panes panes (15) with the others attached and then detached, and beside a
pane flooding --flood-lines lines (3000000) into a bridge whose surface reads slowly:
4 KiB a millisecond, stopping 250 ms every MiB. --flood-surface fast reads it as fast as it
comes instead, so the flood's time is the program's, held only by the link and the daemon's
flow control: how a remote window is judged.

--daemon starts that muster-daemon for the run. --socket measures one already running,
such as a devenv's through a forwarded socket; the bare PTY is still this machine's.

--throughput times a pane with nothing attached draining `yes | head -c --bytes`
(30000000) against a bare PTY read loop draining the same, best of --runs (3).";

/// Roughly a fast typist: back-to-back keys would measure a queue
/// draining rather than what one keystroke at a time sees.
const TYPING_GAP: Duration = Duration::from_millis(150);
const TIMEOUT: Duration = Duration::from_secs(5);
const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

struct Options {
    daemon: Target,
    bridge: PathBuf,
    samples: usize,
    panes: usize,
    flood_lines: u64,
    /// How the flooded pane's surface reads.
    flood_pace: Pace,
    json: bool,
}

enum Target {
    Spawn(PathBuf),
    Socket(PathBuf),
}

impl Options {
    fn parse(arguments: &[String]) -> Option<Options> {
        let (mut daemon, mut bridge, mut json) = (None, None, false);
        let (mut samples, mut panes, mut flood_lines) = (60, 15, 3_000_000);
        let mut flood_pace = Pace::SLOW;
        let mut read = arguments.iter();
        while let Some(flag) = read.next() {
            if flag == "--json" {
                json = true;
                continue;
            }
            let value = read.next()?;
            match flag.as_str() {
                "--daemon" => daemon = Some(Target::Spawn(value.into())),
                "--socket" => daemon = Some(Target::Socket(value.into())),
                "--bridge" => bridge = Some(value.into()),
                "--samples" => samples = value.parse().ok().filter(|&n| n > 0)?,
                "--panes" => panes = value.parse().ok().filter(|&n| n >= 2)?,
                "--flood-lines" => flood_lines = value.parse().ok()?,
                "--flood-surface" => {
                    flood_pace = match value.as_str() {
                        "slow" => Pace::SLOW,
                        "fast" => Pace::FAST,
                        _ => return None,
                    };
                }
                _ => return None,
            }
        }
        Some(Options {
            daemon: daemon?,
            bridge: bridge?,
            samples,
            panes,
            flood_lines,
            flood_pace,
            json,
        })
    }
}

fn throughput_options(arguments: &[String]) -> Option<(PathBuf, u64, usize)> {
    let (mut daemon, mut bytes, mut runs) = (None, 30_000_000, 3);
    let mut read = arguments.iter();
    while let Some(flag) = read.next() {
        let value = read.next()?;
        match flag.as_str() {
            "--daemon" => daemon = Some(PathBuf::from(value)),
            "--bytes" => bytes = value.parse().ok().filter(|&n| n > 0)?,
            "--runs" => runs = value.parse().ok().filter(|&n| n > 0)?,
            _ => return None,
        }
    }
    Some((daemon?, bytes, runs))
}

/// One route from a keystroke to its glyph.
enum Route<'a> {
    Plain(&'a mut Surface),
    Stream(&'a mut Stream, &'a str),
    Bridge(&'a mut Surface, &'a str),
}

impl Route<'_> {
    fn sample(&mut self, daemon: &mut Daemon, letter: u8) -> f64 {
        let typed = Instant::now();
        let shown = match self {
            Route::Plain(surface) => {
                surface.type_letter(letter);
                surface.wait_for(letter, typed, TIMEOUT)
            }
            Route::Stream(stream, pane) => {
                daemon.key(pane, letter);
                stream.wait_for(letter, typed, TIMEOUT)
            }
            Route::Bridge(surface, pane) => {
                daemon.key(pane, letter);
                surface.wait_for(letter, typed, TIMEOUT)
            }
        };
        shown.unwrap_or_else(|| {
            panic!("{:?} never came back within {TIMEOUT:?}", char::from(letter))
        })
    }
}

/// Every path once per sample, in an order that rotates, so load lands on all of them alike.
fn round(daemon: &mut Daemon, paths: &mut [Route<'_>], samples: usize) -> Vec<Vec<f64>> {
    let mut timings = vec![Vec::with_capacity(samples); paths.len()];
    for index in 0..samples {
        let letter = ALPHABET[index % ALPHABET.len()];
        for step in 0..paths.len() {
            let path = (index + step) % paths.len();
            timings[path].push(paths[path].sample(daemon, letter));
            std::thread::sleep(TYPING_GAP / u32::try_from(paths.len()).unwrap_or(1));
        }
    }
    timings
}

fn row(name: &str, samples: &[f64]) -> Row {
    stats::summarize(name, samples).expect("a path that was sampled")
}

fn load() -> String {
    let mut averages = [0.0f64; 3];
    // SAFETY: getloadavg writes at most the three doubles it is given room for.
    unsafe { libc::getloadavg(averages.as_mut_ptr(), 3) };
    format!("load average {:.2} {:.2} {:.2}", averages[0], averages[1], averages[2])
}

#[allow(clippy::cast_precision_loss)]
fn per(total: u64, count: usize) -> f64 {
    total as f64 / count as f64
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().is_some_and(|flag| flag == "--throughput") {
        let Some((daemon, bytes, runs)) = throughput_options(&arguments[1..]) else {
            eprintln!("{USAGE}");
            std::process::exit(2);
        };
        throughput::run(&daemon, bytes, runs);
        return;
    }
    let Some(options) = Options::parse(&arguments) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let scratch = Scratch::new();
    let log = scratch.0.join("bridge.jsonl");
    let mut daemon = match &options.daemon {
        Target::Spawn(binary) => Daemon::spawn(binary),
        Target::Socket(socket) => Daemon::at(socket),
    };
    let mut report = Vec::new();
    let mut verdicts = Vec::new();

    let (idle, bytes) = idle(&mut daemon, &options, &log);
    report.push(("one pane each, idle", idle.clone()));
    let floor = &idle[0];
    verdicts.extend(stats::against_floor("bridge", &idle[2], floor));
    verdicts.push(Verdict {
        target: "one byte on the surface per echoed byte".to_string(),
        measured: format!("{:.2}", bytes.surface),
        met: (bytes.surface - 1.0).abs() < f64::EPSILON,
    });

    report.push(("a full window", crowded(&mut daemon, &options, &log)));

    let flooded = flood(&mut daemon, &options, &log);
    match &flooded.beside {
        Some(beside) => verdicts.extend(stats::beside_flood(&flooded.alone, beside)),
        None => verdicts.push(Verdict {
            target: "echo beside a flood within 1 ms of the same echo alone".to_string(),
            measured: "not sampled: the flood ended first; raise --flood-lines".to_string(),
            met: false,
        }),
    }
    verdicts.push(Verdict {
        target: "a flooded surface catches up with the pane's screen".to_string(),
        measured: flooded.caught_up.clone().unwrap_or_else(|| "it did".to_string()),
        met: flooded.caught_up.is_none(),
    });
    report.push((
        "beside a flood",
        std::iter::once(flooded.alone.clone()).chain(flooded.beside.clone()).collect(),
    ));

    if options.json {
        let sections: Vec<_> = report
            .iter()
            .map(|(title, rows)| {
                serde_json::json!({
                    "section": title,
                    "rows": rows.iter().map(|row| serde_json::json!({
                        "name": row.name, "samples": row.samples, "min_ms": row.min,
                        "median_ms": row.median, "p95_ms": row.p95, "max_ms": row.max,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let json = serde_json::json!({
            "sections": sections,
            "surface_bytes_per_echo": bytes.surface,
            "stream_bytes_per_echo": bytes.stream,
            "flood": { "behind": flooded.behind, "seconds": flooded.seconds },
            "verdicts": verdicts.iter().map(|verdict| serde_json::json!({
                "target": verdict.target, "measured": verdict.measured, "met": verdict.met,
            })).collect::<Vec<_>>(),
        });
        println!("{json}");
        return;
    }
    for (title, rows) in &report {
        println!("{title}\n{}", stats::render(rows));
    }
    println!(
        "{:.2} bytes onto the surface and {:.2} on the stream per echoed byte; the stream's are \
         the byte and its framing.",
        bytes.surface, bytes.stream
    );
    let surface = if options.flood_pace.stall.is_some() {
        "a surface reading 4 KiB a millisecond, stopping 250 ms every MiB"
    } else {
        "a surface reading as fast as it came"
    };
    println!(
        "the flood took {:.1} s through {surface}; its bridge fell behind {} times.\n",
        flooded.seconds, flooded.behind
    );
    println!("against MIP-3 section 13's targets:");
    for verdict in &verdicts {
        println!("  {}", verdict.line());
    }
}

struct Bytes {
    surface: f64,
    stream: f64,
}

/// A directory of this run's own, for the bridges' log, removed when the run ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let path = std::env::temp_dir().join(format!("muster-latency-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn idle(daemon: &mut Daemon, options: &Options, log: &Path) -> (Vec<Row>, Bytes) {
    println!("one pane each, idle ({})", load());
    let mut plain = Surface::plain();
    let (streamed, bridged) = (daemon.pane("cat"), daemon.pane("cat"));
    let mut stream = Stream::attach(daemon.socket(), &streamed);
    let mut bridge = Surface::bridge(&options.bridge, daemon.socket(), &bridged, log);
    let timings = round(
        daemon,
        &mut [
            Route::Plain(&mut plain),
            Route::Stream(&mut stream, &streamed),
            Route::Bridge(&mut bridge, &bridged),
        ],
        options.samples,
    );
    let bytes = Bytes {
        surface: per(bridge.take_bytes(), options.samples),
        stream: per(stream.take_bytes(), options.samples),
    };
    let rows = vec![
        row("plain pty (the floor)", &timings[0]),
        row("daemon: stream responded", &timings[1]),
        row("bridge: glyph on the surface pty", &timings[2]),
    ];
    daemon.close(&streamed);
    daemon.close(&bridged);
    (rows, bytes)
}

fn crowded(daemon: &mut Daemon, options: &Options, log: &Path) -> Vec<Row> {
    let (streamed, bridged) = (daemon.pane("cat"), daemon.pane("cat"));
    let mut stream = Stream::attach(daemon.socket(), &streamed);
    let mut bridge = Surface::bridge(&options.bridge, daemon.socket(), &bridged, log);
    // Paced rather than saturating, as in the Python half: a busy window is agents printing,
    // not `yes`. Twenty lines a second each.
    let others: Vec<String> =
        (2..options.panes).map(|_| daemon.pane("while :; do date; sleep 0.05; done")).collect();
    let samples = (options.samples / 2).max(10);
    let mut rows = Vec::new();
    for attached in [true, false] {
        let surfaces: Vec<_> = if attached {
            others
                .iter()
                .map(|pane| {
                    Surface::bridge(&options.bridge, daemon.socket(), pane, log)
                        .read_in_background(Pace::DRAIN)
                })
                .collect()
        } else {
            Vec::new()
        };
        let which = if attached { "others attached" } else { "others detached" };
        println!("{} panes printing, {which} ({})", options.panes, load());
        std::thread::sleep(Duration::from_secs(1));
        stream.settle();
        bridge.settle();
        let timings = round(
            daemon,
            &mut [Route::Stream(&mut stream, &streamed), Route::Bridge(&mut bridge, &bridged)],
            samples,
        );
        rows.push(row(&format!("stream responded, {which}"), &timings[0]));
        rows.push(row(&format!("bridge glyph, {which}"), &timings[1]));
        drop(surfaces);
    }
    for pane in others.iter().chain([&streamed, &bridged]) {
        daemon.close(pane);
    }
    rows
}

struct Flood {
    alone: Row,
    /// None when the flood ended before the first sample beside it.
    beside: Option<Row>,
    behind: usize,
    seconds: f64,
    /// Why the flooded surface did not end up showing the pane's screen, if it did not.
    caught_up: Option<String>,
}

fn flood(daemon: &mut Daemon, options: &Options, log: &Path) -> Flood {
    const DONE: &str = "flood-done";
    let quiet = daemon.pane("cat");
    let mut bridge = Surface::bridge(&options.bridge, daemon.socket(), &quiet, log);
    println!("one pane alone, then beside a flood ({})", load());
    let alone = round(daemon, &mut [Route::Bridge(&mut bridge, &quiet)], options.samples);

    // Waits for a line before flooding, so the flood starts once its surface is attached.
    let flooding =
        daemon.pane(&format!("read go; seq 1 {}; echo {DONE}; exec cat", options.flood_lines));
    let surface = Surface::bridge(&options.bridge, daemon.socket(), &flooding, log)
        .read_in_background(options.flood_pace);
    let started = Instant::now();
    daemon.send_line(&flooding, "go");
    let mut beside = Vec::new();
    for index in 0..options.samples {
        if surface.shows(DONE) {
            break;
        }
        let letter = ALPHABET[index % ALPHABET.len()];
        beside.push(Route::Bridge(&mut bridge, &quiet).sample(daemon, letter));
        std::thread::sleep(TYPING_GAP);
    }
    let deadline = Instant::now() + Duration::from_mins(10);
    while !surface.shows(DONE) {
        assert!(Instant::now() < deadline, "the flood never finished on its surface");
        std::thread::sleep(Duration::from_millis(100));
    }
    let seconds = started.elapsed().as_secs_f64();
    std::thread::sleep(Duration::from_secs(1));
    let caught_up = compare(&surface.bytes(), &daemon.screen(&flooding));
    let behind = std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains("bridge.behind") && line.contains(&flooding))
        .count();
    daemon.close(&quiet);
    daemon.close(&flooding);
    let alone = row("bridge glyph, alone", &alone[0]);
    let beside = stats::summarize(
        &format!("bridge glyph, beside a flood ({} samples)", beside.len()),
        &beside,
    );
    Flood { alone, beside, behind, seconds, caught_up }
}

/// Whether a surface fed `bytes` shows `screen`, the daemon's own rows; the first difference if
/// not.
fn compare(bytes: &[u8], screen: &[String]) -> Option<String> {
    let (cols, rows) =
        (daemon::GRID.cols.try_into().unwrap(), daemon::GRID.rows.try_into().unwrap());
    let mut terminal = muster_vt::Terminal::new(cols, rows).expect("a terminal");
    terminal.write(bytes);
    let shown: Vec<String> = terminal
        .viewport(cols, rows)
        .rows
        .iter()
        .map(|row| row.text().trim_end().to_string())
        .collect();
    for (index, expected) in screen.iter().enumerate() {
        let actual = shown.get(index).map_or("", String::as_str);
        if actual != expected.trim_end() {
            return Some(format!("row {index} shows {actual:?} where the pane has {expected:?}"));
        }
    }
    None
}
