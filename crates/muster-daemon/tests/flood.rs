//! A pane flooding output cannot delay another pane's echo (MIP-3 sections 4 and 13).
//!
//! What the gate asserts is the structure that makes that true: the flooding pane's bridge is
//! behind and skipped, and another pane's keystroke still comes back as an echo, through a
//! writer, a reader and a stream of its own. How long the echo takes is a machine's measurement
//! rather than a property, and on a machine building several things at once a timing assertion
//! only teaches people to skip the test; `echo_latency` prints the numbers when asked.

mod support;

use std::time::{Duration, Instant};

use muster_harness::Input;
use proto::input_event::{self, Input as Event};
use support::*;

/// libghostty's code for the X key.
const KEY_X: u32 = 43;

fn pane(name: &str, tab: &str, command: &str) -> proto::pane_request::Create {
    proto::pane_request::Create {
        command: Some(command.to_string()),
        grid: Some(proto::Grid { cols: 80, rows: 24, width_px: 800, height_px: 480 }),
        ..create(name, in_new_tab(tab))
    }
}

fn x() -> Event {
    Event::Key(input_event::Key {
        action: proto::KeyAction::Press.into(),
        key: KEY_X,
        text: "x".to_string(),
        ..input_event::Key::default()
    })
}

/// A pane counting as fast as it can with a bridge that never acknowledges, left behind; and a quiet pane
/// echoing what it is typed, with a bridge that keeps up.
struct Panes {
    daemon: Daemon,
    flood: Stream,
    flood_surface: Surface,
    quiet: Stream,
    surface: Surface,
    input: Input,
}

fn flooded() -> Panes {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, pane("flood", "t1", "seq 1 1000000000"));
    make(&mut control, pane("quiet", "t2", "cat"));

    let mut flood = attached(&daemon, "flood", false);
    let mut flooded = Surface::new(80, 24);
    flooded.follow(&mut flood, "the flood to fall behind", false, |surface| surface.behind > 0);

    let mut quiet = attached(&daemon, "quiet", false);
    let mut surface = Surface::new(80, 24);
    surface.follow(&mut quiet, "the quiet pane's replay", true, |surface| surface.replays > 0);
    let input = Input::connect(daemon.socket_path());
    Panes { daemon, flood, flood_surface: flooded, quiet, surface, input }
}

impl Panes {
    /// Types an x into the quiet pane and waits for its echo to come back on its stream.
    fn echo(&mut self) -> Duration {
        let echoed = self.surface.screen().matches('x').count();
        let typed = Instant::now();
        self.input.send("quiet", x());
        self.surface.follow(&mut self.quiet, "the echo", true, |surface| {
            surface.screen().matches('x').count() > echoed
        });
        typed.elapsed()
    }
}

#[test]
fn a_pane_behind_its_flood_does_not_hold_up_another_panes_echo() {
    let mut flooded = flooded();
    for _ in 0..20 {
        flooded.echo();
    }

    // Still behind: the flood's bridge was told once and has had no output since, while the
    // flood's program kept writing and its terminal kept parsing.
    let output = flooded.flood_surface.output;
    while let Some(message) = flooded.flood.next_within(Duration::from_millis(50)) {
        flooded.flood_surface.take(message);
    }
    assert_eq!(flooded.flood_surface.behind, 1);
    assert_eq!(flooded.flood_surface.output, output, "a bridge behind gets no output");
    let mut control = flooded.daemon.connect();
    let counted = |control: &mut Control| -> u64 {
        let text = read_text(control, "flood", 0, 0).text;
        text.lines().filter_map(|line| line.trim().parse().ok()).max().unwrap_or(0)
    };
    let before = counted(&mut control);
    until(
        "the flood's terminal to keep parsing",
        || counted(&mut control) > before,
        || format!("it stood at {before}"),
    );
}

/// Prints echo latency on the quiet pane, alone and beside the flood. Run with
/// `cargo test -p muster-daemon --test flood -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a property: see the module comment"]
fn echo_latency() {
    let quiet_only = {
        let daemon = daemon();
        let mut control = daemon.connect();
        make(&mut control, pane("quiet", "t2", "cat"));
        let mut quiet = attached(&daemon, "quiet", false);
        let mut surface = Surface::new(80, 24);
        surface.follow(&mut quiet, "the replay", true, |surface| surface.replays > 0);
        let mut input = Input::connect(daemon.socket_path());
        (0..200)
            .map(|_| {
                let echoed = surface.screen().matches('x').count();
                let typed = Instant::now();
                input.send("quiet", x());
                surface.follow(&mut quiet, "the echo", true, |surface| {
                    surface.screen().matches('x').count() > echoed
                });
                typed.elapsed()
            })
            .collect::<Vec<_>>()
    };
    let mut flooded = flooded();
    let beside_flood: Vec<Duration> = (0..200).map(|_| flooded.echo()).collect();
    println!("alone:        {}", summary(quiet_only));
    println!("beside flood: {}", summary(beside_flood));
}

fn summary(mut samples: Vec<Duration>) -> String {
    samples.sort();
    let at = |fraction: f64| {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let index = ((samples.len() - 1) as f64 * fraction) as usize;
        samples[index]
    };
    format!(
        "median {:?}, p95 {:?}, max {:?} over {}",
        at(0.5),
        at(0.95),
        samples[samples.len() - 1],
        samples.len()
    )
}
