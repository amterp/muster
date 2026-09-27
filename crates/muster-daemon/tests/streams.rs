//! Stream connections: a bridge attached to a pane gets a replay, then the pane's output, under
//! credit (MIP-3 section 4).

mod support;

use support::*;

fn running(name: &str, tab: &str, command: &str) -> proto::pane_request::Create {
    proto::pane_request::Create {
        command: Some(command.to_string()),
        grid: Some(proto::Grid { cols: 80, rows: 24, width_px: 800, height_px: 480 }),
        ..create(name, in_new_tab(tab))
    }
}

/// A shell loop that waits for `flag` to exist, then runs `then`: how a test tells a pane's
/// program to go on without typing into it.
fn after(flag: &std::path::Path, then: &str) -> String {
    format!("while [ ! -e {} ]; do sleep 0.02; done; {then}", flag.display())
}

fn raise(flag: &std::path::Path) {
    std::fs::write(flag, "").expect("a flag file");
}

#[test]
fn a_bridge_is_brought_to_the_pane_by_a_replay_and_then_follows_it() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let flag = daemon.root().join("go");
    make(
        &mut control,
        running("p1", "t1", &format!("echo before; {}", after(&flag, "echo after"))),
    );
    let shown = until_text(&mut control, "p1", "before");

    let mut stream = attached(&daemon, "p1", false);
    let mut surface = Surface::new(80, 24);
    surface.follow(&mut stream, "the replay", true, |surface| surface.replays > 0);
    assert!(surface.attached.is_some(), "the offset comes before the replay");
    assert_eq!(surface.text().trim_end(), shown.trim_end(), "the replay is the pane's screen");
    assert_eq!(surface.output, 0);

    raise(&flag);
    surface.follow(&mut stream, "output after the replay", true, |surface| {
        surface.screen().contains("after")
    });
    assert!(surface.output > 0, "it came as output, not another replay");

    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    surface.follow(&mut stream, "the pane closing", true, |surface| surface.detached.is_some());
    assert_eq!(surface.detached, Some(proto::DetachReason::Closed));
    surface.follow(&mut stream, "the daemon hanging up", true, |surface| surface.ended);
}

#[test]
fn nothing_is_lost_or_doubled_between_the_replay_and_the_output_after_it() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(
        &mut control,
        running("p1", "t1", "i=0; while [ $i -lt 20000 ]; do echo n$i; i=$((i+1)); done; echo end"),
    );
    // Attached while the loop runs, so the replay holds some rows and output the rest.
    let mut stream = attached(&daemon, "p1", false);
    let mut surface = Surface::new(80, 24);
    surface.follow(&mut stream, "the loop's end", true, |surface| {
        surface.screen().lines().any(|line| line == "end")
    });

    let numbers: Vec<u32> = surface
        .text()
        .lines()
        .filter_map(|line| line.strip_prefix('n').and_then(|number| number.parse().ok()))
        .collect();
    let expected: Vec<u32> = (0..20000).collect();
    assert!(surface.output > 0, "attached after the loop had finished, which proves nothing");
    assert!(numbers == expected, "every row once, in order; {} rows arrived", numbers.len());
}

#[test]
fn a_second_bridge_is_refused_unless_it_takes_over_and_the_first_is_told() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));

    let mut first = attached(&daemon, "p1", false);
    let mut first_surface = Surface::new(80, 24);
    first_surface.follow(&mut first, "the first attach", true, |surface| surface.replays > 0);

    let mut second = attached(&daemon, "p1", false);
    let mut refused = Surface::new(80, 24);
    refused.follow(&mut second, "a refusal", true, |surface| surface.refused.is_some());
    assert!(refused.refused.as_deref().unwrap().contains("takeover"));

    let mut third = attached(&daemon, "p1", true);
    let mut taking = Surface::new(80, 24);
    taking.follow(&mut third, "the takeover's replay", true, |surface| surface.replays > 0);
    first_surface.follow(&mut first, "the displaced bridge told", true, |surface| {
        surface.detached.is_some()
    });
    assert_eq!(first_surface.detached, Some(proto::DetachReason::TakenOver));

    let mut missing = attached(&daemon, "nowhere", false);
    let mut nothing = Surface::new(80, 24);
    nothing.follow(&mut missing, "a refusal", true, |surface| surface.refused.is_some());
    assert!(nothing.refused.as_deref().unwrap().contains("no pane nowhere"));
}

#[test]
fn a_pane_whose_process_ends_detaches_its_bridge_saying_so() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let flag = daemon.root().join("go");
    make(&mut control, running("p1", "t1", &after(&flag, "exit 3")));
    let mut stream = attached(&daemon, "p1", false);
    let mut surface = Surface::new(80, 24);
    surface.follow(&mut stream, "the replay", true, |surface| surface.replays > 0);
    raise(&flag);
    surface.follow(&mut stream, "the pane exiting", true, |surface| surface.detached.is_some());
    assert_eq!(surface.detached, Some(proto::DetachReason::Exited));
}

#[test]
fn a_bridge_resizes_its_pane() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let out = daemon.root().join("size");
    make(
        &mut control,
        running(
            "p1",
            "t1",
            &format!(
                "while :; do stty size > {0}.new; mv {0}.new {0}; sleep 0.05; done",
                out.display()
            ),
        ),
    );
    let mut stream = attached(&daemon, "p1", false);
    stream.resize(proto::Grid { cols: 100, rows: 30, width_px: 1000, height_px: 600 });
    until_some("the program to see its new size", || {
        std::fs::read_to_string(&out).ok().filter(|size| size.trim() == "30 100")
    });
}

#[test]
fn a_bridge_that_stops_acknowledging_falls_behind_and_is_caught_up_with_the_screen() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(
        &mut control,
        running(
            "p1",
            "t1",
            "yes | head -c 3000000; echo; echo flooded; while :; do sleep 1; echo tick; done",
        ),
    );
    let mut stream = attached(&daemon, "p1", false);
    let mut surface = Surface::new(80, 24);
    surface.follow(&mut stream, "falling behind", false, |surface| surface.behind > 0);
    assert!(
        surface.output <= 256 * 1024 + 64 * 1024,
        "at most the window and one read before falling behind, and {} arrived",
        surface.output
    );

    until_text(&mut control, "p1", "flooded");
    let replays = surface.replays;
    surface.follow(&mut stream, "a catch-up", true, |surface| surface.replays > replays);
    assert_eq!(surface.behind, 1, "told once");
    let caught_up = surface.text();
    assert!(caught_up.contains("flooded"), "the catch-up shows the screen: {caught_up:?}");
    assert!(
        caught_up.len() < 1_000_000,
        "the catch-up carries the screen, not the history the surface missed"
    );

    let output = surface.output;
    surface.follow(&mut stream, "output again", true, |surface| surface.output > output);
}
