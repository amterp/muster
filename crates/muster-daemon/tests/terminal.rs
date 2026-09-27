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
