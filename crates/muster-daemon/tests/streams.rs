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

#[test]
fn a_closed_pane_lets_go_of_its_terminal_even_when_its_bridge_has_stopped_reading() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let done = daemon.root().join("done");
    // Ignores the hangup a close sends and writes until its terminal is gone for good, which is
    // when the daemon closes the pane's master. The shell records that itself, with a builtin:
    // dash, Debian's /bin/sh, never finishes starting another program once its terminal is gone.
    let flood = format!(
        "trap '' HUP; while printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; do :; done; \
         : > {}",
        done.display()
    );
    make(&mut control, running("p1", "t1", &flood));
    // Attached, and never read: its socket fills, and the daemon's writes to it block.
    let _stream = attached(&daemon, "p1", false);
    std::thread::sleep(std::time::Duration::from_millis(500));

    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    until(
        "the pane's program to find its terminal closed",
        || done.exists(),
        || "the daemon still holds the pane's master".to_string(),
    );
}

/// A bridge that stops acknowledging holds its program for a grace period and no longer: then it
/// is behind, and the program goes on.
#[test]
fn a_bridge_that_stops_acknowledging_is_behind_after_a_grace_and_the_program_goes_on() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let flag = daemon.root().join("go");
    make(
        &mut control,
        running("p1", "t1", &after(&flag, "yes | head -c 3000000; echo; echo flooded")),
    );
    let mut stream = attached(&daemon, "p1", false);
    raise(&flag);
    // Timed on the raw frames: parsing them as it went would take long enough to hide the wait.
    // What is timed is the silence between the last output sent and the notice: the program
    // waiting for credit that never comes.
    let (mut last, mut owed) = (None, 0);
    let held = loop {
        match stream.next_within(muster_harness::PATIENCE) {
            Some(Some(proto::stream_message::Message::Output(bytes))) => {
                last = Some(std::time::Instant::now());
                owed += bytes.len() as u64;
            }
            Some(Some(proto::stream_message::Message::Behind(_))) => {
                break last.expect("output before falling behind").elapsed();
            }
            Some(Some(_)) => {}
            Some(None) | None => panic!("never behind"),
        }
    };
    assert!(
        held >= std::time::Duration::from_millis(80),
        "behind {held:?} after the last output: the program should have waited out a grace \
         for credit first"
    );
    stream.credit(owed);
    until_text(&mut control, "p1", "flooded");
    let mut surface = Surface::new(80, 24);
    surface.follow(&mut stream, "the catch-up", true, |surface| surface.replays > 0);
    assert_eq!(surface.behind, 0, "told once");
}

/// A bridge that fell behind is caught up only once half its window is free, not at the window's
/// edge, so a slow bridge crediting in batches is not flipped between the two: each flip is a
/// catch-up composed under the pane's lock.
#[test]
fn a_slow_bridge_crediting_in_batches_is_not_thrashed() {
    const WINDOW: u64 = 256 * 1024;
    const BATCH: u64 = WINDOW / 8;
    let daemon = daemon();
    let mut control = daemon.connect();
    let flag = daemon.root().join("go");
    make(
        &mut control,
        running("p1", "t1", &after(&flag, "yes | head -c 3000000; echo; echo flooded")),
    );
    let mut stream = attached(&daemon, "p1", false);
    raise(&flag);

    let (mut received, mut credited, mut owed) = (0, 0, 0);
    // Once a window has arrived the bridge stops crediting until it is told it is behind, which
    // a stall past the grace brings; then it credits a batch at a time.
    let (mut holding, mut behind, mut tail) = (false, false, Vec::new());
    // What was unacknowledged when each catch-up came.
    let mut caught_up = Vec::new();
    let deadline = std::time::Instant::now() + muster_harness::PATIENCE;
    while !String::from_utf8_lossy(&tail).contains("flooded") {
        assert!(std::time::Instant::now() < deadline, "the flood never ended");
        match stream.next_within(std::time::Duration::from_millis(100)) {
            Some(Some(proto::stream_message::Message::Output(bytes))) => {
                received += bytes.len() as u64;
                owed += bytes.len() as u64;
                tail.extend_from_slice(&bytes);
            }
            Some(Some(proto::stream_message::Message::Behind(_))) => {
                (holding, behind) = (false, true);
            }
            Some(Some(proto::stream_message::Message::Replay(bytes))) if behind => {
                behind = false;
                caught_up.push(received - credited);
                tail.extend_from_slice(&bytes);
            }
            Some(None) => panic!("the daemon hung up"),
            _ => {}
        }
        tail.drain(..tail.len().saturating_sub(4096));
        if caught_up.is_empty() && !behind && received >= WINDOW {
            holding = true;
        }
        if holding {
            continue;
        }
        // Behind, every byte sent before the notice has arrived and nothing more comes until the
        // catch-up, so one batch at a time shows which credit brings it.
        let batches = if behind { 1 } else { owed / BATCH };
        for _ in 0..batches.min(owed / BATCH) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            stream.credit(BATCH);
            credited += BATCH;
            owed -= BATCH;
        }
    }
    assert!(!caught_up.is_empty(), "the stall never put the bridge behind");
    assert!(
        caught_up.iter().all(|&unacknowledged| unacknowledged <= WINDOW / 2),
        "caught up with {caught_up:?} bytes unacknowledged; not before half the window is free"
    );
}

/// A bridge across a slow link asks for a larger window, and gets it within the daemon's bounds.
#[test]
fn a_bridge_is_sent_the_window_it_asked_for_before_it_falls_behind() {
    const KIB: u64 = 1024;
    for (asked, window) in [(None, 256 * KIB), (Some(1024 * KIB), 1024 * KIB), (Some(1), 64 * KIB)]
    {
        let daemon = daemon();
        let mut control = daemon.connect();
        let flag = daemon.root().join("go");
        make(&mut control, running("p1", "t1", &after(&flag, "yes | head -c 3000000")));
        let mut stream = Stream::connect(daemon.socket_path());
        stream.attach_with_window("p1", None, false, asked);
        raise(&flag);
        let mut sent = 0;
        loop {
            match stream.next_within(muster_harness::PATIENCE) {
                Some(Some(proto::stream_message::Message::Output(bytes))) => {
                    sent += bytes.len() as u64;
                }
                Some(Some(proto::stream_message::Message::Behind(_))) => break,
                Some(Some(_)) => {}
                Some(None) | None => panic!("never behind"),
            }
        }
        // Output goes while there is room, so the last read may overshoot the window.
        assert!(
            (window..window + 64 * KIB).contains(&sent),
            "asked for {asked:?}: sent {sent} bytes before falling behind, not a {window}-byte window"
        );
    }
}
