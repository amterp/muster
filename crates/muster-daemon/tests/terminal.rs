//! Each pane's headless terminal: what `pane read` reads, what answers a program's queries,
//! and what turns its output's effects into events (MIP-3 section 7).

mod support;

use proto::event::Event as Payload;
use proto::pane_effect::Effect as Shown;
use proto::session_request;
use support::*;

fn grid(cols: u32, rows: u32, width_px: u32, height_px: u32) -> proto::Grid {
    proto::Grid { cols, rows, width_px, height_px }
}

fn running(name: &str, tab: &str, command: String) -> proto::pane_request::Create {
    proto::pane_request::Create {
        command: Some(command),
        grid: Some(grid(80, 24, 800, 480)),
        ..create(name, in_new_tab(tab))
    }
}

fn set_palette(background: u32, scheme: proto::ColorScheme) -> proto::request::Service {
    session(session_request::Request::SetPalette(proto::SetPalette {
        palette: Some(proto::Palette {
            entries: Vec::new(),
            foreground: 0xff_ff_ff,
            background,
            cursor: None,
            scheme: scheme.into(),
        }),
    }))
}

#[test]
fn a_pane_is_read_in_pages_counted_from_its_oldest_row() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let mut asked = running(
        "p1",
        "t1",
        "i=0; while [ $i -lt 60 ]; do echo row$i; i=$((i+1)); done".to_string(),
    );
    asked.grid = Some(grid(40, 10, 0, 0));
    make(&mut control, asked);

    let all = until_text(&mut control, "p1", "row59");
    let first = all.lines().position(|line| line == "row0").expect("the first row is held");
    let first = first as u64;

    let page = read_text(&mut control, "p1", first + 10, 2);
    assert_eq!(page.text, "row10\nrow11", "history the screen scrolled past");
    assert_eq!(page.first_row, first + 10);
    assert!(page.total_rows >= first + 60, "{} rows in all", page.total_rows);

    let tail = read_text(&mut control, "p1", first + 58, 0);
    assert!(tail.text.starts_with("row58\nrow59"), "zero rows reads to the end: {:?}", tail.text);

    let past = read_text(&mut control, "p1", page.total_rows + 5, 10);
    assert_eq!(past.text, "", "a page past the end is empty");
    expect(&mut control, read_request("missing", 0, 1), proto::Outcome::NotThere);
}

/// The most text one page holds, well under the largest message a client accepts.
const PAGE_BYTES: usize = 4 << 20;

#[test]
fn a_long_history_is_read_in_pages_no_larger_than_a_message_may_be() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let set_scrollback = session(session_request::Request::SetScrollback(proto::SetScrollback {
        bytes: Some(1 << 30),
    }));
    expect(&mut control, set_scrollback, proto::Outcome::Done);
    // About 6.4 MB of text, one 70-character row per line.
    let lines = 90_000;
    let command = format!(
        "awk 'BEGIN {{ for (i = 0; i < {lines}; i++) printf \"line%06d %s\\n\", i, \
         \"{}\" }}'; echo finished",
        "x".repeat(59)
    );
    make(&mut control, running("p1", "t1", command));
    until_some("the pane to finish printing", || {
        let total = read_text(&mut control, "p1", 0, 1).total_rows;
        let tail = read_text(&mut control, "p1", total.saturating_sub(30), 0).text;
        tail.contains("finished").then_some(())
    });

    let whole = read_text(&mut control, "p1", 0, 0);
    assert!(whole.text.len() <= PAGE_BYTES, "a page of {} bytes", whole.text.len());
    assert_eq!(whole.rows as usize, whole.text.split('\n').count(), "the page says its rows");

    let mut seen = Vec::new();
    let mut next = 0;
    while next < whole.total_rows {
        let page = read_text(&mut control, "p1", next, 0);
        assert!(page.text.len() <= PAGE_BYTES, "a page of {} bytes", page.text.len());
        assert!(page.rows > 0, "a page before the end holds something");
        seen.extend(page.text.split('\n').filter_map(|line| {
            line.strip_prefix("line").and_then(|rest| rest[..6].parse::<usize>().ok())
        }));
        next = page.first_row + u64::from(page.rows);
    }
    assert_eq!(seen, (0..lines).collect::<Vec<_>>(), "every line once, in order");
}

#[test]
fn the_daemon_answers_a_programs_queries() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let cases: [(&str, &str, &[u8]); 6] = [
        ("da1", "\\033[c", b"\x1b[?62;22;52c"),
        ("da2", "\\033[>c", b"\x1b[>1;10;0c"),
        ("cursor", "\\033[3;5H\\033[6n", b"\x1b[3;5R"),
        ("size", "\\033[14t", b"\x1b[4;480;800t"),
        ("kitty", "\\033[?u", b"\x1b[?0u"),
        // XTGETTCAP for the terminal's name, which has to be the TERM the pane was given.
        ("name", "\\033P+q544e\\033\\\\", b"\x1bP1+r544E=787465726D2D67686F73747479\x1b\\"),
    ];
    for (index, (name, query, _)) in cases.iter().enumerate() {
        let out = daemon.root().join(name);
        make(
            &mut control,
            running(&format!("p{index}"), &format!("t{index}"), answer_to(query, &out)),
        );
    }
    for (name, query, expected) in cases {
        let answered = bytes_in(&daemon.root().join(name));
        assert_eq!(
            String::from_utf8_lossy(&answered),
            String::from_utf8_lossy(expected),
            "the answer to {name} ({query})"
        );
    }
}

#[test]
fn colors_come_from_the_apps_palette_and_clipboard_access_from_its_setting() {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, set_palette(0x10_20_30, proto::ColorScheme::Dark), proto::Outcome::Done);
    let deny = session(session_request::Request::SetClipboardWrite(proto::SetClipboardWrite {
        allowed: false,
    }));
    expect(&mut control, deny, proto::Outcome::Done);

    let background = daemon.root().join("background");
    make(&mut control, running("p1", "t1", answer_to("\\033]11;?\\033\\\\", &background)));
    let attributes = daemon.root().join("attributes");
    make(&mut control, running("p2", "t2", answer_to("\\033[c", &attributes)));

    let answered = String::from_utf8_lossy(&bytes_in(&background)).into_owned();
    assert!(answered.contains("rgb:1010/2020/3030"), "OSC 11 answered {answered:?}");
    assert_eq!(bytes_in(&attributes), b"\x1b[?62;22c", "no clipboard access claimed");
}

#[test]
fn a_program_that_asked_is_told_when_the_appearance_turns() {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, set_palette(0, proto::ColorScheme::Dark), proto::Outcome::Done);
    let out = daemon.root().join("scheme");
    let command = format!(
        "printf '\\033[?2031h'; echo ready; stty raw -echo min 0 time 100; \
         dd bs=64 count=1 of={} 2>/dev/null",
        out.display()
    );
    make(&mut control, running("p1", "t1", command));
    until_text(&mut control, "p1", "ready");

    expect(&mut control, set_palette(0xff_ff_ff, proto::ColorScheme::Light), proto::Outcome::Done);
    assert_eq!(bytes_in(&out), b"\x1b[?997;2n");
}

#[test]
fn what_a_programs_output_asks_for_arrives_as_events() {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    let command = "printf '\\033]2;hello\\033\\\\\\a\\033]9;done\\033\\\\\\033]9;4;1;50\\033\\\\\
                   \\033]52;c;aGk=\\033\\\\\\033]7;file://localhost/private/tmp\\033\\\\'"
        .to_string();
    make(&mut control, running("p1", "t1", command));

    let heard = |events: &[proto::Event]| {
        let mut title = None;
        let mut cwd = None;
        let mut shown = Vec::new();
        for event in events {
            match event.event.as_ref() {
                Some(Payload::PaneChanged(changed)) => {
                    let pane = changed.pane.as_ref().unwrap();
                    if !pane.title.is_empty() {
                        title = Some(pane.title.clone());
                    }
                    if pane.cwd == "/private/tmp" {
                        cwd = Some(pane.cwd.clone());
                    }
                }
                Some(Payload::PaneEffect(effect)) => shown.extend(effect.effect.clone()),
                _ => {}
            }
        }
        (title, cwd, shown)
    };
    let events = events_until(&mut control, "a title, a directory and four effects", |events| {
        let (title, cwd, shown) = heard(events);
        title.is_some() && cwd.is_some() && shown.len() >= 4
    });
    let (title, _, effects) = heard(&events);
    assert_eq!(title.as_deref(), Some("hello"));

    assert!(effects.iter().any(|effect| matches!(effect, Shown::Bell(_))), "{effects:?}");
    assert!(
        effects.iter().any(|effect| matches!(effect, Shown::Notification(n) if n.body == "done")),
        "{effects:?}"
    );
    assert!(
        effects.iter().any(|effect| matches!(effect,
            Shown::Progress(p) if p.state() == proto::ProgressState::Set && p.percent == Some(50))),
        "{effects:?}"
    );
    assert!(
        effects.iter().any(|effect| matches!(effect,
            Shown::ClipboardWrite(write) if write.text == "hi")),
        "{effects:?}"
    );
}

#[test]
fn a_shell_that_does_not_report_its_directory_is_followed_anyway() {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    let elsewhere = daemon.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).expect("a scratch directory");
    let elsewhere = canonical(&elsewhere).display().to_string();
    make(&mut control, running("p1", "t1", format!("cd {elsewhere} && echo moved")));

    events_until(&mut control, "the pane's new directory", |events| {
        events.iter().any(|event| {
            matches!(event.event.as_ref(), Some(Payload::PaneChanged(changed))
                if changed.pane.as_ref().is_some_and(|pane| pane.cwd == elsewhere))
        })
    });
    let snapshot = snapshot(&mut control);
    assert_eq!(snapshot.panes[0].cwd, elsewhere);
}

/// A replay carries no kitty images, so a surface that attaches has none of them. The daemon's
/// terminal forgets them too, and a program placing one by id is told it is gone, as a fresh
/// terminal would tell it, and sends it again.
#[test]
fn a_kitty_image_is_forgotten_once_a_replay_is_sent() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let (before, after, go) =
        (daemon.root().join("before"), daemon.root().join("after"), daemon.root().join("go"));
    let transmit = "\\033_Ga=t,f=32,s=1,v=1,i=7,q=2;AAAA/w==\\033\\\\";
    let place = "\\033_Ga=p,i=7\\033\\\\";
    let script = format!(
        "{}; printf '{place}'; dd bs=4096 count=1 of={} 2>/dev/null; while [ ! -e {} ]; do \
         sleep 0.05; done; printf '{place}'; dd bs=4096 count=1 of={} 2>/dev/null",
        answer_to(transmit, &before).split("; dd").next().unwrap(),
        before.display(),
        go.display(),
        after.display(),
    );
    make(&mut control, running("p1", "t1", script));
    let placed = String::from_utf8_lossy(&bytes_in(&before)).into_owned();
    assert!(placed.contains("i=7;OK"), "placed before the replay: {placed:?}");

    let mut stream = attached(&daemon, "p1", false);
    let mut surface = Surface::new(80, 24);
    surface.follow(&mut stream, "the replay", true, |surface| surface.replays > 0);
    std::fs::write(&go, "").unwrap();

    let placed = String::from_utf8_lossy(&bytes_in(&after)).into_owned();
    assert!(placed.contains("i=7;ENOENT"), "placed after the replay: {placed:?}");
}

/// XTVERSION is answered as Ghostty answers it, with the version a pane's TERM_PROGRAM_VERSION
/// gives, so a program asking either way hears the same.
#[test]
fn xtversion_is_answered_as_ghostty_with_term_program_version() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let (version, answer) = (daemon.root().join("version"), daemon.root().join("answer"));
    let script = format!(
        "printf %s \"$TERM_PROGRAM_VERSION\" > {}; {}",
        version.display(),
        answer_to("\\033[>q", &answer)
    );
    make(&mut control, running("p1", "t1", script));
    let version = String::from_utf8(bytes_in(&version)).unwrap();
    let answered = String::from_utf8_lossy(&bytes_in(&answer)).into_owned();
    assert_eq!(answered, format!("\x1bP>|ghostty {version}\x1b\\"));
}

fn clear_screen(daemon: &Daemon, pane: &str) {
    use proto::input_event::{Input as Event, Perform, perform};
    let mut input = muster_harness::Input::connect(daemon.socket_path());
    input.send(
        pane,
        Event::Perform(Perform {
            action: Some(perform::Action::ClearScreen(perform::ClearScreen {})),
        }),
    );
}

/// Ghostty's clear_screen at a shell's prompt: the history goes, the screen is scrolled away,
/// as Ghostty's own erase does at a prompt, and the shell is sent a form feed to draw its prompt
/// again. The surface drawing the pane is sent the cleared screen.
#[test]
fn clear_screen_at_a_prompt_clears_everything_and_asks_the_shell_to_redraw() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let heard = daemon.root().join("heard");
    // Output past the screen's height, then a prompt marked as shell integration marks one.
    let script = format!(
        "i=0; while [ $i -lt 40 ]; do echo old$i; i=$((i+1)); done; printf '\\033]133;A\\007$ '; \
         stty raw -echo min 1 time 0; dd bs=1 count=1 of={} 2>/dev/null; sleep 30",
        heard.display()
    );
    make(&mut control, running("p1", "t1", script));
    until_text(&mut control, "p1", "old39\n$");
    let mut stream = attached(&daemon, "p1", false);
    let mut surface = Surface::new(80, 24);
    surface.follow(&mut stream, "the replay", true, |surface| surface.replays > 0);
    assert!(surface.screen().contains("old39"));

    clear_screen(&daemon, "p1");

    assert_eq!(bytes_in(&heard), b"\x0c", "the shell is asked to redraw its prompt");
    let total = read_text(&mut control, "p1", 0, 0).total_rows;
    assert!(total < 64, "the history before the clear is gone: {total} rows");
    let screen = read_text(&mut control, "p1", total - 24, 0);
    assert!(!screen.text.contains("old"), "nothing old on the screen: {:?}", screen.text);
    surface.follow(&mut stream, "the cleared screen", true, |surface| surface.replays > 1);
    assert!(!surface.screen().contains("old"), "the surface is cleared too");
}

/// Away from a prompt, clear_screen erases history and the rows above the cursor, and tells the
/// program nothing.
#[test]
fn clear_screen_away_from_a_prompt_keeps_the_cursor_row_and_tells_nobody() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let heard = daemon.root().join("heard");
    let script = format!(
        "i=0; while [ $i -lt 40 ]; do echo old$i; i=$((i+1)); done; printf 'here'; \
         stty raw -echo min 0 time 20; dd bs=1 count=1 of={} 2>/dev/null; sleep 30",
        heard.display()
    );
    make(&mut control, running("p1", "t1", script));
    until_text(&mut control, "p1", "here");

    clear_screen(&daemon, "p1");

    until_some("the clear", || {
        let text = read_text(&mut control, "p1", 0, 0);
        (!text.text.contains("old")).then_some(text)
    });
    let text = read_text(&mut control, "p1", 0, 0);
    assert_eq!(text.total_rows, 24, "no history is left");
    assert!(text.text.contains("here"), "the cursor's row stays: {:?}", text.text);
    // The program read for two seconds and heard nothing.
    until_some("the program to stop reading", || heard.exists().then_some(()));
    assert_eq!(std::fs::read(&heard).unwrap(), b"");
}
