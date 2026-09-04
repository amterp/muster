//! What a window's answer looks like, for a person and for a program.
//!
//! Two renderings of one answer, and neither is the schema. The messages are the contract between
//! the app and this CLI; how they read is this CLI's own decision, which is why `--json` is a
//! reshaping rather than a dump - a program wants one flat list of panes it can filter, and the
//! wire shape nests them under daemons and tabs because that is how a window is built.
//!
//! Colour is written unconditionally and stripped on the way out. `main` wraps stdout in an
//! [`anstream`] stream, which keeps the escapes when a person is looking and removes them when the
//! output is a pipe or a file, or when `NO_COLOR` is set - so nothing here has to know which it is,
//! and no agent reading a pipe ever sees an escape in the middle of a pane name.

use std::collections::BTreeMap;

use anstyle::{AnsiColor, Style};
use muster_proto::{Response, Window, response};
use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;

use crate::{Trouble, dial};

/// How a refusal is marked, here so that `report` and this file agree on it.
pub const ERROR: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Red))).bold();

const NAME: Style = Style::new().bold();
const QUIET: Style = Style::new().dimmed();
const PLAIN: Style = Style::new();

/// Turns an answer into what should be printed, or into the reason there is nothing to print.
pub fn answer(response: &Response, json: bool) -> Result<String, Trouble> {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => Err(Trouble::Refused(failure.reason.clone())),
        Some(response::Payload::Ok(_)) => {
            // Nothing to say. Silence is the answer to a request that did what it was asked, and
            // JSON gets an object anyway so that `--json` always produces one.
            Ok(if json { json!({ "ok": true }).to_string() } else { String::new() })
        }
        Some(response::Payload::Made(made)) => Ok(if json {
            json!({ "pane": made.pane_id }).to_string()
        } else {
            // The name alone, with nothing around it, because the next line of a script is
            // `muster pane send --pane "$(muster pane new --down)"`.
            made.pane_id.clone()
        }),
        Some(response::Payload::Window(window)) => {
            Ok(if json { window_json(window).to_string() } else { window_text(window) })
        }
        Some(response::Payload::PaneText(read)) => Ok(if json {
            json!({ "text": read.text, "rows": read.rows, "truncated": read.truncated }).to_string()
        } else {
            // The text and nothing else. Whatever a pane printed is what somebody asked for,
            // and a row count printed under it would be this CLI writing into an answer a
            // reader is about to grep. `--json` is where the two facts beside it live.
            read.text.clone()
        }),
        Some(response::Payload::Daemons(daemons)) => {
            Ok(if json { daemons_json(daemons).to_string() } else { daemons_text(daemons) })
        }
        Some(other) => Err(Trouble::Refused(format!(
            "the window answered with {}, which nothing here asked for. That is a bug in muster \
             rather than anything to do with the request.",
            named(other)
        ))),
        None => Err(Trouble::Refused(
            "the window answered with an empty message. That is a bug in muster: every request \
             has an answer, even a refusal."
                .to_string(),
        )),
    }
}

/// What a payload is called, for the one message that says a build disagrees with itself.
fn named(payload: &response::Payload) -> &'static str {
    match payload {
        response::Payload::Ok(_) => "an ok",
        response::Payload::Failure(_) => "a refusal",
        response::Payload::Attached(_) => "an attached pane",
        response::Payload::Bindings(_) => "a list of key bindings",
        response::Payload::Appearance(_) => "an appearance",
        response::Payload::Findings(_) => "search results",
        response::Payload::Window(_) => "a window",
        response::Payload::Made(_) => "a pane",
        response::Payload::WindowFrame(_) => "a window frame",
        response::Payload::PaneText(_) => "a pane's text",
        response::Payload::PaneViewport(_) => "where a pane is looking",
        response::Payload::Daemons(_) => "a list of daemons",
    }
}

/// Every window on this machine, one row each.
///
/// A summary rather than each window's whole answer: what somebody running this wants is which
/// window is which, and the `--socket` to reach it with. `muster window --socket <path>` is the
/// whole of it after that.
pub fn windows(
    answers: &[(String, Result<Response, Trouble>)],
    here: Option<&str>,
    json: bool,
) -> String {
    if json {
        let listed: Vec<Value> = answers
            .iter()
            .map(|(path, answer)| {
                let mut row = json!({ "socket": path, "here": here == Some(path.as_str()) });
                match answer.as_ref().map(|response| response.payload.as_ref()) {
                    Ok(Some(response::Payload::Window(window))) => {
                        row["panes"] = json!(counted_panes(window));
                        row["tabs"] = json!(counted_tabs(window));
                        row["keyboard"] = json!(keyboard_pane(window));
                    }
                    // Listed with its reason rather than dropped: a window that is there and
                    // will not answer is the case somebody running this is looking for.
                    Ok(_) => row["unreadable"] = json!("the window answered with something else"),
                    Err(trouble) => row["unreadable"] = json!(trouble.detail()),
                }
                row
            })
            .collect();
        return json!({ "windows": listed }).to_string();
    }

    if answers.is_empty() {
        return styled("no Muster window is listening", QUIET);
    }
    answers
        .iter()
        .map(|(path, answer)| {
            // The pane the keyboard is on, because a person picking between two windows knows
            // them by what they were doing in one - not by a pid.
            let summary = match answer.as_ref().map(|response| response.payload.as_ref()) {
                Ok(Some(response::Payload::Window(window))) => {
                    format!("{} panes, {} tabs", counted_panes(window), counted_tabs(window))
                }
                Ok(_) => "answered with something else".to_string(),
                Err(trouble) => trouble.detail().to_string(),
            };
            let mark = if here == Some(path.as_str()) { "▸" } else { " " };
            format!("{mark} {} {}", styled(path, NAME), styled(&summary, QUIET))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every window's whole answer, one after another, when the caller narrowed to none of them.
///
/// Distinct from [`windows`] above, which is `window list` and is a summary: this is what
/// `muster window` prints, asked of each window that answered. So the two surfaces stay what they
/// were - one says which windows there are, the other says what they are showing - and neither
/// grew the other's job.
///
/// Headed by the pid, because a window has no name a person chose and a socket path is a
/// temporary directory with a pid at the end of it. The path is beside it in both shapes, since
/// that is what `--socket` takes and what the next command needs.
pub fn answers(answers: &[(String, Result<Response, Trouble>)], json: bool) -> String {
    if json {
        let listed: Vec<Value> = answers
            .iter()
            .map(|(path, answer)| {
                let mut row = json!({ "socket": path, "window": dial::named_window(path) });
                match answer {
                    Ok(response) => match &response.payload {
                        Some(response::Payload::Window(window)) => {
                            // Flattened into the row rather than nested under a key, so that one
                            // window's answer is the same object here as it is on its own and a
                            // filter written for one reads across all of them:
                            // `.windows[].panes[] | select(.state == "blocked")`.
                            if let Value::Object(fields) = window_json(window) {
                                for (key, value) in fields {
                                    row[key] = value;
                                }
                            }
                        }
                        _ => row["unreadable"] = json!(named_or_empty(response)),
                    },
                    Err(trouble) => row["unreadable"] = json!(trouble.detail()),
                }
                row
            })
            .collect();
        return json!({ "windows": listed }).to_string();
    }

    answers
        .iter()
        .map(|(path, answer)| {
            let heading = format!(
                "{} {}",
                styled(&format!("window {}", dial::named_window(path).unwrap_or(path)), NAME),
                styled(path, QUIET)
            );
            let body = match answer {
                Ok(response) => match &response.payload {
                    Some(response::Payload::Window(window)) => window_text(window),
                    _ => styled(named_or_empty(response), QUIET),
                },
                Err(trouble) => styled(trouble.detail(), QUIET),
            };
            format!("{heading}\n{}", body.trim_end())
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What a window answered with, when it was not what was asked for.
///
/// Listed rather than dropped, on the same terms as `window list`: a window that is there and
/// answers something else is exactly what somebody running this is trying to find out about.
fn named_or_empty(response: &Response) -> &'static str {
    match response.payload.as_ref() {
        Some(payload) => named(payload),
        None => "an empty message",
    }
}

fn counted_panes(window: &Window) -> usize {
    tabs(window).map(|tab| tab.panes.len()).sum()
}

fn counted_tabs(window: &Window) -> usize {
    tabs(window).count()
}

/// Every tab the window holds, in the order it walks them.
fn tabs(window: &Window) -> impl Iterator<Item = &muster_proto::RosterTab> {
    window.roster.iter().flat_map(|roster| roster.tabs.iter())
}

/// A window as somebody reads it: its tabs, the panes in them, then the machines behind them.
///
/// Tabs first because that is the window: it holds an ordered list of them and shows one, so a
/// person reading this is reading the thing they navigate. The machines follow rather than
/// heading the list, because a tab may hold panes on two of them and grouping by machine would
/// describe a window that no longer exists (MIP-2).
///
/// Which machine holds a pane is on the pane's own row, and only while more than one is
/// attached. On one machine the answer is on every row and says nothing.
fn window_text(window: &Window) -> String {
    let keyboard = keyboard_pane(window);
    let states = states(window);
    let widths = Widths::across(window, &states);
    let machines: Vec<&muster_proto::Machine> = window.daemons.iter().collect();
    let say_machine = machines.len() > 1;

    let mut lines: Vec<String> = Vec::new();
    for tab in tabs(window) {
        lines.push(tab_line(&widths, tab));
        for pane in &tab.panes {
            lines.push(pane_line(
                &widths,
                pane,
                states.get(pane.pane_id.as_str()).copied().unwrap_or("unknown"),
                keyboard.as_deref() == Some(pane.pane_id.as_str()),
                say_machine,
            ));
        }
    }

    // The machines, whether or not they are holding anything. A machine holding nothing is the
    // one worth seeing here: nothing above mentions it, and it is still attached and still
    // somewhere a pane can be made (`muster pane new --daemon <id>`).
    for machine in machines {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(daemon_lines(machine));
    }

    if lines.is_empty() {
        return "no daemon is attached, so this window is showing nothing".to_string();
    }
    lines.join("\n")
}

/// A machine's heading, and beneath it what would end with the daemon behind it.
///
/// Two lines rather than one, and the second is the one this was missing. A person deciding
/// whether a herdr process is safe to end could not pair a pid with the work it holds - herdr
/// answers no question that gets from one to the other - so the choice was made on age, and
/// age picks the wrong process (kan a_28YghIUw2). Muster started the daemon or chose to attach
/// to it, so it can simply say.
fn daemon_lines(machine: &muster_proto::Machine) -> Vec<String> {
    let mut lines = vec![daemon_line(&machine.daemon_id, &machine.state, &machine.detail)];
    let where_it_runs =
        if machine.host.is_empty() { "this machine".to_string() } else { machine.host.clone() };
    // Whether Muster started it, because that is the difference between ending something this
    // window made and ending something it found somebody else's agents already inside.
    let whose = if machine.started_by_muster { "started by Muster" } else { "already running" };
    let holding = match (machine.panes, machine.directories.as_slice()) {
        (0, _) => "no panes".to_string(),
        (1, [only]) => format!("1 pane in {only}"),
        (count, []) => format!("{count} panes"),
        (count, directories) => format!("{count} panes in {}", directories.join(", ")),
    };
    lines.push(styled(&format!("  {where_it_runs} · {whose} · {holding}"), QUIET));
    lines.push(styled(&format!("  {}", machine.socket), QUIET));
    lines
}

fn daemon_line(daemon: &str, state: &str, detail: &str) -> String {
    let mut line = format!("{}  {}", styled(daemon, NAME), styled(state, health_style(state)));
    if !detail.is_empty() {
        line.push_str("  ");
        line.push_str(&styled(detail, QUIET));
    }
    line
}

/// One tab's row: its place, its name, what to call it, and whether anything is showing it.
///
/// Place then name, the order a pane row uses. Not aligned with the pane rows under it, which are
/// a level in rather than a column beside; padded so that the labels line up between tabs when two
/// daemons spell their ids to different lengths.
///
/// The name is what `muster tab` takes, which is why it is drawn at all - a row somebody can read
/// and not act on is what this said before tabs were named.
fn tab_line(widths: &Widths, tab: &muster_proto::RosterTab) -> String {
    let mut line = format!(
        "{} {}  {}",
        styled("tab", QUIET),
        tab.place,
        pad(&tab.tab_id, widths.name, PLAIN, Align::Left),
    );
    if !tab.label.is_empty() {
        line.push_str("  ");
        line.push_str(&tab.label);
    }
    if tab.on_screen {
        line.push_str("  ");
        line.push_str(&styled("on screen", QUIET));
    }
    line
}

/// One pane's row: which one has the keyboard in the gutter, then place, name, state and label.
///
/// The keyboard is marked in the gutter rather than in a column of its own, the way any list marks
/// the item you are on - a column would put eight blank spaces on every other row. Being off screen
/// is a note after the label instead, because most panes are on screen and the exception is the
/// thing worth saying.
fn pane_line(
    widths: &Widths,
    pane: &muster_proto::RosterPane,
    state: &str,
    has_keyboard: bool,
    say_machine: bool,
) -> String {
    let mut line = format!(
        "  {}{}  {}  {}  {}",
        if has_keyboard { styled("▸", NAME) + " " } else { "  ".to_string() },
        pad(&pane.place.to_string(), widths.place, QUIET, Align::Right),
        pad(&pane.pane_id, widths.name, PLAIN, Align::Left),
        pad(state, widths.state, agent_style(state), Align::Left),
        pane.label,
    );
    if !pane.subtitle.is_empty() {
        line.push_str(&styled(&format!(" · {}", pane.subtitle), QUIET));
    }
    if !pane.on_screen {
        line.push_str(&styled("  (hidden)", QUIET));
    }
    // Last on the row and only with more than one machine attached, because a tab may span two
    // and the pane is the only thing that can say which. On one machine it is on every row and
    // says nothing.
    if say_machine {
        line.push_str(&styled(&format!("  ({})", pane.daemon_id), QUIET));
    }
    line
}

/// What each word means at a glance, so a window with fifteen panes can be read without counting.
///
/// The legend is the window's rather than this file's: `PaneAppearance.borderColor` in
/// `Sources/MusterMac/PaneChrome.swift` decides it and `docs/architecture.md` states it with the
/// reasoning. The two disagreed once - working green here and blue there, done the other way about -
/// and the cost was not a wrong pixel but a person learning the colours could not be trusted.
/// Nothing checks them across the language line, so a row that moves here moves in both.
///
/// Blocked is yellow because the sixteen have no orange: the medium's limit, not a second opinion.
/// And what is named is a slot rather than a pixel, so a user who repaints `[colors] palette`
/// repaints the legend, which is theirs to do.
///
/// Working is cyan rather than the blue it was, and the argument for moving it is partly this
/// file's own: plain ANSI blue is the least legible of the sixteen on a dark background, and
/// working is the state a window spends most of its time in.
///
/// Only the states worth a colour get one. `unknown` is the ordinary answer for a pane running a
/// shell rather than an agent, and colouring it would put a signal on almost every row; `idle` goes
/// bare for the reason the window draws it no border, that no colour is what resting looks like in
/// a list of words and the row already prints the word.
fn agent_style(state: &str) -> Style {
    match state {
        "working" => Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Cyan))),
        "blocked" => Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Yellow))),
        "done" => Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Green))),
        _ => QUIET,
    }
}

/// A stale mirror looks exactly like a live one, so the one word that says which is coloured for it.
fn health_style(state: &str) -> Style {
    match state {
        "connected" => Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Green))),
        "stale" => Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Yellow))),
        _ => ERROR,
    }
}

#[derive(Debug, Clone, Copy)]
enum Align {
    Left,
    Right,
}

fn styled(text: &str, style: Style) -> String {
    format!("{}{text}{}", style.render(), style.render_reset())
}

/// A styled cell padded to a column, measured as a terminal would measure it.
///
/// By display width rather than by character count, because a pane called `🤖 A` is three columns
/// wide and two characters long - and a whole window of agents named that way would step one
/// column further out of line per row.
fn pad(text: &str, width: usize, style: Style, align: Align) -> String {
    let blank = " ".repeat(width.saturating_sub(text.width()));
    match align {
        Align::Left => format!("{}{}", styled(text, style), blank),
        Align::Right => format!("{}{}", blank, styled(text, style)),
    }
}

/// How wide each column has to be to hold every row.
///
/// Measured over the whole window rather than per tab, so the columns line up down the page - the
/// list is read as one list even though it is drawn in groups. `name` measures tab names as well as
/// pane names, so that a window whose two daemons spell their ids to different lengths still lines
/// its labels up.
struct Widths {
    place: usize,
    name: usize,
    state: usize,
}

impl Widths {
    fn across(window: &Window, states: &BTreeMap<&str, &str>) -> Widths {
        let mut widths = Widths { place: 1, name: 0, state: 0 };
        for tab in tabs(window) {
            widths.name = widths.name.max(tab.tab_id.width());
            for pane in &tab.panes {
                widths.place = widths.place.max(pane.place.to_string().width());
                widths.name = widths.name.max(pane.pane_id.width());
                let state = states.get(pane.pane_id.as_str()).copied().unwrap_or("unknown");
                widths.state = widths.state.max(state.width());
            }
        }
        widths
    }
}

/// A window as a program reads it: one flat list of panes, each carrying where it sits.
///
/// Flat because that is what filtering wants - "every blocked pane" is one pass here and a nested
/// walk on the wire shape - and each pane names its daemon and its tab so nothing is lost by it.
fn window_json(window: &Window) -> Value {
    let keyboard = keyboard_pane(window);
    let states = states(window);

    let mut tabs_out = Vec::new();
    let mut panes = Vec::new();
    for tab in tabs(window) {
        tabs_out.push(json!({
            "tab": tab.tab_id,
            // The machines this tab holds panes on, in the order their regions sit on screen.
            // Plural because a tab may span two, which is the whole of what changed here: a
            // script that read `daemon` off a tab was reading a question the tab no longer
            // answers on its own.
            "daemons": tab.daemon_ids,
            "place": tab.place,
            "label": tab.label,
            "given_name": tab.given_name,
            "on_screen": tab.on_screen,
        }));
        for pane in &tab.panes {
            panes.push(json!({
                "pane": pane.pane_id,
                "place": pane.place,
                "daemon": pane.daemon_id,
                "tab": tab.tab_id,
                "label": pane.label,
                "given_name": pane.given_name,
                "subtitle": pane.subtitle,
                "state": states.get(pane.pane_id.as_str()).copied().unwrap_or("unknown"),
                "on_screen": pane.on_screen,
                "keyboard": keyboard.as_deref() == Some(pane.pane_id.as_str()),
            }));
        }
    }

    let daemons: Vec<Value> = window
        .daemons
        .iter()
        .map(|daemon| {
            json!({
                "daemon": daemon.daemon_id,
                "state": daemon.state,
                "detail": daemon.detail,
                "host": daemon.host,
                "socket": daemon.socket,
                "started_by_muster": daemon.started_by_muster,
                "panes": daemon.panes,
                "directories": daemon.directories,
            })
        })
        .collect();

    // A pane says which tab holds it by name rather than by place, so one read is enough to act
    // on it: `.panes[] | select(.pane == $MUSTER_PANE) | .tab` is how a pane finds its own tab,
    // and there is nothing in a pane's environment that says. The place is still in `tabs[]` for
    // anyone who wants it.
    json!({
        "daemons": daemons,
        "keyboard": keyboard,
        // Which tab the window is on, named once rather than repeated on every region below.
        // `tabs[].on_screen` says the same thing and needs a scan to find it.
        "showing": showing(window),
        "regions": regions(window),
        "tabs": tabs_out,
        "panes": panes,
    })
}

/// Which tab the window is showing, or null when it is showing none.
fn showing(window: &Window) -> Option<&str> {
    let tab = window.view.as_ref()?.tab_id.as_str();
    (!tab.is_empty()).then_some(tab)
}

/// The parts of the tab on screen, left to right, one per machine holding panes in it.
///
/// JSON only, and one region for every tab until somebody groups two. A person reads `on_screen`
/// on a tab and has the window in front of them; a script arranging one has neither, and nothing
/// else in the answer says how wide each machine's half is or which order they sit in. This is
/// the only part of the arrangement Muster owns outright - no daemon knows another one exists.
///
/// Which tab these divide is `showing` above rather than a key on every row, because they all
/// show the same one.
fn regions(window: &Window) -> Vec<Value> {
    let Some(view) = window.view.as_ref() else { return Vec::new() };
    view.regions
        .iter()
        .map(|region| {
            json!({
                "region": region.region_id,
                "daemon": region.daemon_id,
                "pane": region.pane_id,
                // A share of the sum rather than a fraction of the window, which is how the
                // core holds it: every region starts at 1, so three untouched regions read as
                // 1, 1, 1 rather than as three thirds that have to add up.
                "weight": region.weight,
                "keyboard": region.region_id == view.focused_region,
                // Whether this region is filled by one pane rather than by its tab's whole
                // tree. Reported because it is the difference between a tab holding one pane
                // and a tab whose others are hidden, and nothing else in this answer implies
                // it: every pane in the tab still lists, and the ones a zoom is covering read
                // as `on_screen: false` exactly like the panes of a tab in the background.
                "zoomed": region.zoomed,
            })
        })
        .collect()
}

/// Which pane this window's keyboard is on, by way of the region that has it.
fn keyboard_pane(window: &Window) -> Option<String> {
    let view = window.view.as_ref()?;
    let region = view.regions.iter().find(|region| region.region_id == view.focused_region)?;
    Some(region.pane_id.clone()).filter(|pane| !pane.is_empty())
}

/// What each pane's agent is doing, by pane.
///
/// Keyed by pane alone even though the messages carry a daemon too: a pane name is Muster's own and
/// unique across every machine in the window, which is the whole reason a caller can address one
/// without knowing where it lives.
fn states(window: &Window) -> BTreeMap<&str, &str> {
    window.panes.iter().map(|pane| (pane.pane_id.as_str(), pane.state.as_str())).collect()
}

/// The daemons on this machine, for somebody deciding which of them to end.
///
/// Every row carries what it holds and how to end it, because those are the two things the
/// decision needs and neither is recoverable from a process list. A count is not enough: of
/// twenty daemons on one machine, nineteen held nothing and one held somebody's live agent.
///
/// Nothing here sorts by age or suggests a candidate. Age is exactly what picked the wrong
/// process on that machine, and a tool that nominated one to kill would be a reaper with extra
/// steps.
fn daemons_text(daemons: &muster_proto::Daemons) -> String {
    if !daemons.remembered {
        return "Muster has nowhere to write down the daemons it starts, so there is no record \
                to read. This says nothing about what is running on this machine."
            .to_string();
    }
    if daemons.daemons.is_empty() {
        return "Muster has started no daemon on this machine that it still has a record of."
            .to_string();
    }

    let mut lines = Vec::new();
    for daemon in &daemons.daemons {
        let here = if daemon.attached_here { " · this window" } else { "" };
        lines.push(format!("{}{}{}{here}", NAME.render(), described(daemon), NAME.render_reset()));
        lines.push(format!("  {}{}{}", QUIET.render(), daemon.socket, QUIET.render_reset()));
    }
    lines.push(String::new());
    lines.push(format!(
        "{}End one with: HERDR_SOCKET_PATH=<socket> herdr server stop{}",
        QUIET.render(),
        QUIET.render_reset()
    ));
    lines.join("\n")
}

/// One daemon as a headline: what state it is in, and what it holds if it would say.
fn described(daemon: &muster_proto::KnownDaemon) -> String {
    match daemon.state.as_str() {
        "answering" if daemon.panes == 0 => "answering · holding nothing".to_string(),
        "answering" => format!(
            "answering · {} pane(s){}",
            daemon.panes,
            if daemon.directories.is_empty() {
                String::new()
            } else {
                format!(" in {}", daemon.directories.join(", "))
            }
        ),
        // Both of the not-answering cases say what to conclude, because the conclusions differ
        // and neither is obvious from the word alone.
        "silent" => "silent · its socket file is there and nothing answers on it, so this daemon \
                     has ended and left the file behind"
            .to_string(),
        "gone" => "gone · no socket file left, so it either ended or is running with its socket \
                   path deleted out from under it, and nothing here can tell those apart"
            .to_string(),
        other => format!("{other} · a state this muster does not know, so the window is newer"),
    }
}

fn daemons_json(daemons: &muster_proto::Daemons) -> Value {
    json!({
        "remembered": daemons.remembered,
        "daemons": daemons
            .daemons
            .iter()
            .map(|daemon| json!({
                "socket": daemon.socket,
                "started": daemon.started,
                "state": daemon.state,
                "panes": daemon.panes,
                "directories": daemon.directories,
                "attached_here": daemon.attached_here,
            }))
            .collect::<Vec<Value>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::{QUIET, agent_style};
    use anstyle::{AnsiColor, Color, Style};

    fn hue(color: AnsiColor) -> Style {
        Style::new().fg_color(Some(Color::Ansi(color)))
    }

    /// The window decides the legend and nothing checks the two across the language line, so this
    /// is the tripwire. A failure here means `muster window` and the window itself contradict each
    /// other about what the product's own vocabulary looks like, which is how somebody learns to
    /// stop trusting the colours. Move `Sources/MusterMac/PaneChrome.swift` and
    /// `docs/architecture.md` with it, or move it back.
    #[test]
    fn the_legend_matches_the_window() {
        assert_eq!(agent_style("working"), hue(AnsiColor::Cyan), "the window paints working cyan");
        assert_eq!(
            agent_style("blocked"),
            hue(AnsiColor::Yellow),
            "the window paints blocked orange, and yellow is the nearest of the sixteen"
        );
        assert_eq!(agent_style("done"), hue(AnsiColor::Green), "the window paints done green");
    }

    /// Idle and unknown are the resting answer and the row already prints the word, so neither
    /// earns a hue. The invented state is this file's half of the window's rule that a state we
    /// could not read is unknown and never idle: bare says nothing, where a hue would say
    /// something wrong.
    #[test]
    fn resting_states_carry_no_hue() {
        assert_eq!(agent_style("idle"), QUIET);
        assert_eq!(agent_style("unknown"), QUIET);
        assert_eq!(agent_style("compacting"), QUIET);
    }
}
