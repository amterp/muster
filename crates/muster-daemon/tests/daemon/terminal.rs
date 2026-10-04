//! Each pane's headless terminal: what `pane read` reads, what answers a program's queries,
//! and what turns its output's effects into events (MIP-3 section 7).

use crate::support::*;
use proto::event::Event as Payload;
use proto::pane_effect::Effect as Shown;
use proto::session_request;

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

/// The newest rows are read without the history before them or the blank rest of the screen
/// beneath: a reader asking for the last few rows of a pane at a prompt is sent those.
#[test]
fn the_last_rows_end_at_the_last_row_with_anything_on_it() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let command = format!("{HUNDRED_ROWS}; clear; echo marker; echo prompt; exec cat");
    let mut asked = running("p1", "t1", command);
    asked.grid = Some(grid(40, 10, 0, 0));
    make(&mut control, asked);
    until_text(&mut control, "p1", "prompt");

    let last = |control: &mut Control, count: u32| {
        let read = pane(proto::pane_request::Request::Read(proto::pane_request::Read {
            pane: "p1".to_string(),
            last: count,
            ..Default::default()
        }));
        match expect(control, read, proto::Outcome::Done).answer.detail {
            Some(proto::answer::Detail::Text(text)) => text,
            other => panic!("a read answered with {other:?}"),
        }
    };
    let newest = last(&mut control, 2);
    assert_eq!(newest.text, "marker\nprompt", "{newest:?}");
    assert_eq!(newest.rows, 2);
    assert!(
        newest.first_row + 2 < newest.total_rows,
        "the blank rest of the screen is still held, just not read: {newest:?}"
    );

    let more = last(&mut control, 1000);
    assert!(more.text.ends_with("marker\nprompt"), "as many as there are: {more:?}");
    assert_eq!(more.first_row, more.oldest_row, "from the oldest row `clear` left");
}

/// Rows `row0` to `row99`, printed by a pane's program.
const HUNDRED_ROWS: &str = "i=0; while [ $i -lt 100 ]; do echo row$i; i=$((i+1)); done";

fn has_row(text: &str, row: &str) -> bool {
    text.lines().any(|line| line == row)
}

/// A program on the alternate screen is read as what it shows, and the history of the screen
/// under it comes back when it leaves.
#[test]
fn a_program_on_the_alternate_screen_is_read_without_the_history_beneath_it() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let flag = daemon.root().join("leave");
    let command = format!(
        "{HUNDRED_ROWS}; printf '\\033[?1049h'; echo alternate-one; echo alternate-two; \
         while [ ! -e {0} ]; do sleep 0.02; done; printf '\\033[?1049l'; echo left",
        flag.display()
    );
    make(&mut control, running("p1", "t1", command));

    let alternate = until_text(&mut control, "p1", "alternate-two");
    assert!(!has_row(&alternate, "row0") && !has_row(&alternate, "row99"), "{alternate:?}");

    std::fs::write(&flag, "").unwrap();
    let main = until_text(&mut control, "p1", "left\n");
    assert!(has_row(&main, "row0") && has_row(&main, "row99"), "{main:?}");
}

/// A program that erases the scrollback leaves a pane holding only its screen.
#[test]
fn erased_scrollback_is_gone_from_a_read() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let command = format!("{HUNDRED_ROWS}; printf '\\033[3J'; echo erased");
    make(&mut control, running("p1", "t1", command));

    let text = until_text(&mut control, "p1", "erased\n");
    assert!(!has_row(&text, "row0") && !has_row(&text, "row50"), "{text:?}");
    // What was printed after the erase, and the shell's prompt, may scroll a row or two back in.
    let read = read_text(&mut control, "p1", 0, 0);
    let rows = read.total_rows - read.oldest_row;
    assert!(rows < 24 + 3, "the screen's rows and next to nothing else, not {rows}");
    assert!(
        read.oldest_row > 0 && read.text.starts_with(&format!("row{}\n", read.oldest_row)),
        "rows keep the numbers they had before the erase: {read:?}"
    );
}

/// A line longer than the pane is read as the rows it wrapped onto, since a read counts rows as
/// the screen does (docs/cli/limits.md).
#[test]
fn a_long_line_is_read_as_the_rows_it_wraps_onto() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, running("p1", "t1", "printf '%0167d\\n' 0; echo printed".to_string()));

    let text = until_text(&mut control, "p1", "printed\n");
    let widths: Vec<usize> = text
        .lines()
        .filter(|line| !line.is_empty() && line.chars().all(|character| character == '0'))
        .map(str::len)
        .collect();
    assert_eq!(widths, [80, 80, 7], "{text:?}");
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

/// A multiplier of zero or less would stop the wheel reaching programs at all, and one that
/// is not a number would scroll by nothing anyone could predict.
#[test]
fn a_scroll_multiplier_that_cannot_scale_is_refused() {
    let daemon = daemon();
    let mut control = daemon.connect();
    for multiplier in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let set = proto::SetScrollMultiplier { multiplier };
        let request = session(session_request::Request::SetScrollMultiplier(set));
        expect(&mut control, request, proto::Outcome::Refused);
    }
    let set = proto::SetScrollMultiplier { multiplier: 1.0 };
    let request = session(session_request::Request::SetScrollMultiplier(set));
    expect(&mut control, request, proto::Outcome::AlreadySo);
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
    let made = make(&mut control, running("p1", "t1", command));

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
    let events = events_until_from(
        &mut control,
        made.events,
        "a title, a directory and four effects",
        |events| {
            let (title, cwd, shown) = heard(events);
            title.is_some() && cwd.is_some() && shown.len() >= 4
        },
    );
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
    let made = make(&mut control, running("p1", "t1", format!("cd {elsewhere} && echo moved")));

    events_until_from(&mut control, made.events, "the pane's new directory", |events| {
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
    clear_screen_with_key(daemon, pane, None);
}

/// clear_screen from the key whose binding asked for it.
fn clear_screen_with_key(daemon: &Daemon, pane: &str, key: Option<proto::input_event::Key>) {
    use proto::input_event::{Input as Event, Perform, perform};
    let mut input = muster_harness::Input::connect(daemon.socket_path());
    input.send(
        pane,
        Event::Perform(Perform {
            action: Some(perform::Action::ClearScreen(perform::ClearScreen {})),
            key,
        }),
    );
}

/// On the alternate screen Ghostty leaves the key to the program, so a clear_screen sent there
/// reaches the program as the key, encoded against the program's own keyboard modes - here the
/// kitty protocol's, under which cmd+k is a sequence rather than nothing.
#[test]
fn clear_screen_on_the_alternate_screen_sends_the_program_the_key() {
    const KEY_K: u32 = 30;
    const MODS_SUPER: u32 = 8;
    let daemon = daemon();
    let mut control = daemon.connect();
    let heard = daemon.root().join("heard");
    let script = format!(
        "printf 'before\\n\\033[?1049h\\033[>1uvim'; stty raw -echo min 1 time 0; \
         dd bs=64 count=1 of={} 2>/dev/null; sleep 30",
        heard.display()
    );
    make(&mut control, running("p1", "t1", script));
    until_text(&mut control, "p1", "vim");

    let key = proto::input_event::Key {
        action: proto::KeyAction::Press.into(),
        key: KEY_K,
        mods: MODS_SUPER,
        unshifted_codepoint: u32::from('k'),
        ..proto::input_event::Key::default()
    };
    clear_screen_with_key(&daemon, "p1", Some(key));

    assert_eq!(bytes_in(&heard), b"\x1b[107;9u", "the program gets the key, as it asked keys sent");
    assert!(read_text(&mut control, "p1", 0, 0).text.contains("vim"), "nothing was cleared");
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
    let read = read_text(&mut control, "p1", 0, 0);
    let (total, held) = (read.total_rows, read.total_rows - read.oldest_row);
    assert!(held < 64, "the history before the clear is gone: {held} rows");
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
    assert_eq!(text.total_rows - text.oldest_row, 24, "no history is left");
    assert!(text.text.contains("here"), "the cursor's row stays: {:?}", text.text);
    // The program read for two seconds and heard nothing.
    until_some("the program to stop reading", || heard.exists().then_some(()));
    assert_eq!(std::fs::read(&heard).unwrap(), b"");
}

/// What a program says with XTSHIFTESCAPE reaches the pane's record, so the app can leave
/// shift-clicks to the program as Ghostty would.
#[test]
fn xtshiftescape_is_on_the_panes_record() {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    let made = make(&mut control, running("p1", "t1", "printf '\\033[>1s'; sleep 30".to_string()));

    let captured = |event: &proto::Event| {
        matches!(&event.event, Some(Payload::PaneChanged(changed))
            if changed.pane.as_ref().is_some_and(|pane| pane.shift_capture == Some(true)))
    };
    let changed =
        events_until_from(&mut control, made.events, "shift capture on the record", |events| {
            events.iter().any(captured)
        });
    assert!(!changed.is_empty());
    let record = snapshot(&mut control).panes.into_iter().find(|pane| pane.pane == "p1").unwrap();
    assert_eq!(record.shift_capture, Some(true));
}
