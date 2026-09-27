//! What a program's output asks of the daemon: answers to its queries, and effects.
//!
//! The daemon is the only thing that answers a pane's queries (MIP-3 section 7), so a
//! query libghostty-vt stays silent on is a program waiting forever - and several use a
//! device-attributes query as a fence, waiting on the answer before drawing anything.

use std::sync::{Arc, Mutex};

use muster_vt::{
    Answers, ClipboardLocation, ColorScheme, Effect, Progress, Terminal, TerminalOptions,
};

/// What one write produced, as owned values.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Reply(Vec<u8>),
    Bell,
    Title(String),
    Pwd(String),
    Clipboard(ClipboardLocation, Vec<(String, Vec<u8>)>),
    Notification(String, String),
    Progress(Progress),
}

fn recording(options: TerminalOptions) -> (Terminal, Arc<Mutex<Vec<Seen>>>) {
    let mut terminal = Terminal::with_options(options).expect("libghostty-vt gives us a terminal");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    terminal.set_effect_handler(move |effect| {
        let owned = match effect {
            Effect::Reply(bytes) => Seen::Reply(bytes.to_vec()),
            Effect::Bell => Seen::Bell,
            Effect::Title(title) => Seen::Title(title.to_string()),
            Effect::Pwd(pwd) => Seen::Pwd(pwd.to_string()),
            Effect::ClipboardWrite { location, contents } => Seen::Clipboard(
                location,
                contents.iter().map(|c| (c.mime.to_string(), c.data.to_vec())).collect(),
            ),
            Effect::Notification { title, body } => {
                Seen::Notification(title.to_string(), body.to_string())
            }
            Effect::Progress(progress) => Seen::Progress(progress),
        };
        sink.lock().expect("no handler panicked holding the lock").push(owned);
    });
    (terminal, seen)
}

fn after(options: TerminalOptions, bytes: &[u8]) -> Vec<Seen> {
    let (mut terminal, seen) = recording(options);
    terminal.write(bytes);
    seen.lock().expect("the lock is free").clone()
}

fn replies(options: TerminalOptions, bytes: &[u8]) -> String {
    after(options, bytes)
        .into_iter()
        .filter_map(|seen| match seen {
            Seen::Reply(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            _ => None,
        })
        .collect()
}

fn grid() -> TerminalOptions {
    TerminalOptions::new(80, 24)
}

#[test]
fn device_attributes_answer_as_ghostty_does() {
    assert_eq!(replies(grid(), b"\x1b[c"), "\x1b[?62;22;52c");
    assert_eq!(replies(grid(), b"\x1b[>c"), "\x1b[>1;10;0c");
}

#[test]
fn a_cursor_position_report_comes_back_as_a_reply() {
    assert_eq!(replies(grid(), b"abc\x1b[6n"), "\x1b[1;4R");
}

#[test]
fn xtversion_names_what_the_daemon_says_it_is() {
    let options = TerminalOptions {
        answers: Answers { version: "muster 0.9.0".into(), ..Answers::default() },
        ..grid()
    };
    assert_eq!(replies(options, b"\x1b[>q"), "\x1bP>|muster 0.9.0\x1b\\");
}

#[test]
fn xtgettcap_reports_the_terminfo_name_the_pane_runs_as() {
    let options = TerminalOptions { terminfo_name: Some("xterm-ghostty".into()), ..grid() };
    let name: String = "xterm-ghostty"
        .bytes()
        .flat_map(|b| format!("{b:02X}").into_bytes())
        .map(char::from)
        .collect();
    // "TN", hex-encoded, is the capability's name.
    assert_eq!(replies(options, b"\x1bP+q544e\x1b\\"), format!("\x1bP1+r544E={name}\x1b\\"));
}

#[test]
fn size_reports_use_the_pixel_size_a_surface_gave() {
    let options = TerminalOptions { cell_pixels: (9, 18), ..grid() };
    assert_eq!(replies(options.clone(), b"\x1b[16t"), "\x1b[6;18;9t");
    assert_eq!(replies(options, b"\x1b[18t"), "\x1b[8;24;80t");
    // With no pixel size yet, no answer rather than one claiming zero-pixel cells.
    assert_eq!(replies(grid(), b"\x1b[16t"), "");
}

#[test]
fn the_color_scheme_query_answers_with_the_apps_appearance() {
    let dark = TerminalOptions {
        answers: Answers { color_scheme: Some(ColorScheme::Dark), ..Answers::default() },
        ..grid()
    };
    assert_eq!(replies(dark, b"\x1b[?996n"), "\x1b[?997;1n");
    assert_eq!(replies(grid(), b"\x1b[?996n"), "", "unknown appearance: no answer");
}

#[test]
fn bell_title_and_directory() {
    let seen = after(grid(), b"\x07\x1b]2;building\x1b\\\x1b]7;file://host/tmp\x1b\\");
    assert_eq!(
        seen,
        [Seen::Bell, Seen::Title("building".into()), Seen::Pwd("file://host/tmp".into())]
    );
}

#[test]
fn desktop_notifications_by_either_protocol() {
    assert_eq!(
        after(grid(), b"\x1b]9;done\x1b\\"),
        [Seen::Notification(String::new(), "done".into())]
    );
    assert_eq!(
        after(grid(), b"\x1b]777;notify;Claude;needs you\x1b\\"),
        [Seen::Notification("Claude".into(), "needs you".into())]
    );
}

#[test]
fn progress_reports() {
    assert_eq!(after(grid(), b"\x1b]9;4;1;40\x1b\\"), [Seen::Progress(Progress::Set(40))]);
    assert_eq!(after(grid(), b"\x1b]9;4;3\x1b\\"), [Seen::Progress(Progress::Indeterminate)]);
    assert_eq!(after(grid(), b"\x1b]9;4;0\x1b\\"), [Seen::Progress(Progress::Remove)]);
}

#[test]
fn a_clipboard_write_arrives_decoded() {
    assert_eq!(
        after(grid(), b"\x1b]52;c;aGVsbG8=\x1b\\"),
        [Seen::Clipboard(
            ClipboardLocation::Standard,
            vec![("text/plain".into(), b"hello".to_vec())]
        )]
    );
}

#[test]
fn a_program_cannot_read_its_title_back_into_its_input() {
    // CSI 21 t would echo the title into the program's input, where a title the program
    // chose becomes a command the user never typed.
    assert_eq!(replies(grid(), b"\x1b]2;rm -rf ~\x1b\\\x1b[21t"), "");
}

#[test]
fn a_handler_that_panics_loses_its_effect_not_the_daemon() {
    let mut terminal = Terminal::new(80, 24).expect("libghostty-vt gives us a terminal");
    terminal.set_effect_handler(|_| panic!("a handler bug"));
    terminal.write(b"\x07\x1b[c");
    terminal.write(b"still parsing");
    assert!(
        String::from_utf8_lossy(&terminal.format(muster_vt::FormatOptions::plain()))
            .contains("still parsing")
    );
}

#[test]
fn nothing_is_heard_without_a_handler() {
    let mut terminal = Terminal::new(80, 24).expect("libghostty-vt gives us a terminal");
    terminal.write(b"\x07\x1b[c\x1b]2;title\x1b\\");
    assert_eq!(terminal.title(), "title");
}

#[test]
fn a_terminal_with_a_handler_moves_to_another_thread() {
    let (mut terminal, seen) = recording(grid());
    std::thread::spawn(move || terminal.write(b"\x07")).join().expect("the thread finishes");
    assert_eq!(*seen.lock().expect("the lock is free"), [Seen::Bell]);
}

#[test]
fn images_named_by_a_path_are_refused_and_inline_ones_accepted() {
    // A path names a file on the daemon's machine, which a surface on another machine cannot
    // read (MIP-3 section 4). Refusing it makes a program fall back to sending the image
    // inline, which every surface can draw.
    let query = |medium: &str, payload: &str| {
        replies(
            grid(),
            format!("\x1b_Ga=q,i=31,s=1,v=1,f=24,t={medium};{payload}\x1b\\").as_bytes(),
        )
    };
    let path = "L3RtcC94LnBuZw=="; // "/tmp/x.png"
    for medium in ["f", "t", "s"] {
        assert_eq!(
            query(medium, path),
            "\x1b_Gi=31;EINVAL: unsupported medium\x1b\\",
            "medium {medium}"
        );
    }
    assert_eq!(query("d", "AAAA"), "\x1b_Gi=31;OK\x1b\\");
}

/// A replay can land between two chunks of an image the program is still sending. Forgetting
/// the images then would drop the chunks already here and fail the rest, so an image that is
/// still arriving is kept, and the program hears it arrived.
#[test]
fn forgetting_kitty_images_spares_one_still_arriving() {
    let mut options = grid();
    options.kitty_image_bytes = Some(1 << 20);
    let (mut terminal, seen) = recording(options);
    terminal.write(b"\x1b_Ga=t,f=32,s=1,v=1,i=7,m=1;AAAA\x1b\\");
    terminal.forget_kitty_images();
    terminal.write(b"\x1b_Gm=0;/w==\x1b\\");
    let replies: String = seen
        .lock()
        .expect("the lock is free")
        .iter()
        .filter_map(|seen| match seen {
            Seen::Reply(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
            _ => None,
        })
        .collect();
    assert!(replies.contains("i=7;OK"), "the image arrived whole: {replies:?}");
}
