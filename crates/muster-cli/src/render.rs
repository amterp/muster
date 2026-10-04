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

use crate::Trouble;
use crate::diagram::{self, Lines, Part};

/// How a refusal is marked, here so that `report` and this file agree on it.
pub const ERROR: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Red))).bold();

const NAME: Style = Style::new().bold();
const QUIET: Style = Style::new().dimmed();
const PLAIN: Style = Style::new();

/// Turns an answer into what should be printed, or into the reason there is nothing to print.
pub fn answer(response: &Response, json: bool) -> Result<String, Trouble> {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => Err(Trouble::Refused(failure.reason.clone())),
        // The same exit as a window that took the request and never answered, one hop further
        // in: the window's daemon took it and never answered, so a retry may repeat it.
        Some(response::Payload::Unanswered(unanswered)) => {
            Err(Trouble::Unanswered(unanswered.reason.clone()))
        }
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
        // The pane it went to, as `Made` prints one. Nothing when nothing was asking, which is no
        // failure: `[ -n "$(muster focus --asking)" ]` is how a script asks whether it went.
        Some(response::Payload::Asking(went)) => Ok(match (json, went.pane_id.is_empty()) {
            (true, true) => json!({ "pane": null }).to_string(),
            (true, false) => json!({ "pane": went.pane_id, "daemon": went.daemon_id }).to_string(),
            (false, _) => went.pane_id.clone(),
        }),
        // The same shape as `Asking`: the pane, or nothing at either end of the history.
        Some(response::Payload::Went(went)) => Ok(match (json, went.pane_id.is_empty()) {
            (true, true) => json!({ "pane": null }).to_string(),
            (true, false) => json!({ "pane": went.pane_id, "daemon": went.daemon_id }).to_string(),
            (false, _) => went.pane_id.clone(),
        }),
        Some(response::Payload::Window(window)) => Ok(if json {
            window_json(window, Others::All).to_string()
        } else {
            window_text(window, now_ms(), Others::All, true)
        }),
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
        // One line each, because a watch is read a line at a time as it arrives. The pane's name
        // first, so `grep --line-buffered p1w3r07bsd` is a filter.
        Some(response::Payload::PaneState(agent)) => Ok(if json {
            json!({
                "pane": agent.pane_id,
                "daemon": agent.daemon_id,
                "state": agent.state,
                "since": since_json(agent.since_ms),
                "label": agent.label,
            })
            .to_string()
        } else {
            // The label last, since it is the one field that can hold a space.
            let state = styled(&agent.state, agent_style(&agent.state));
            format!("{}  {state}  {}", agent.pane_id, agent.label).trim_end().to_string()
        }),
        Some(response::Payload::PaneClosed(closed)) => Ok(if json {
            json!({ "pane": closed.pane_id, "daemon": closed.daemon_id, "closed": true })
                .to_string()
        } else {
            format!("{}  {}", closed.pane_id, styled("closed", QUIET))
        }),
        // A daemon on a watch, which says whether the lines about its panes can be believed. The
        // same keys as `daemons[]` in `muster window --json`, and no `pane`, which is how a reader
        // tells it from a pane's line.
        Some(response::Payload::BackendHealth(health)) => Ok(if json {
            json!({ "daemon": health.daemon_id, "state": health.state, "detail": health.detail })
                .to_string()
        } else {
            format!("{}  {}", health.daemon_id, styled(&health.state, health_style(&health.state)))
        }),
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

/// What this machine's daemon holds, answered in a window's place when no window did.
///
/// A window's answer with what only a window has left out - places, the keyboard, what is on
/// screen - and headed with which daemon answered, so nobody reads it as a window's.
/// `answered_by` in the JSON says the same, and a window's own answer never carries it.
pub fn daemon_window(window: &Window, socket: &str, json: bool) -> String {
    if json {
        let mut answer = window_json(window, Others::None);
        if let Value::Object(fields) = &mut answer {
            fields.insert("answered_by".to_string(), json!("daemon"));
            fields.insert("name".to_string(), Value::Null);
        }
        return answer.to_string();
    }
    let heading =
        styled(&format!("no window answered; the muster-daemon at {socket} holds:"), QUIET);
    format!("{heading}\n{}", window_text(window, now_ms(), Others::None, false))
}

/// What a payload is called, for the one message that says a build disagrees with itself.
fn named(payload: &response::Payload) -> &'static str {
    match payload {
        response::Payload::Ok(_) => "an ok",
        response::Payload::Failure(_) => "a refusal",
        response::Payload::Attached(_) => "an attached pane",
        response::Payload::Bindings(_) => "a list of key bindings",
        response::Payload::Appearance(_) => "an appearance",
        response::Payload::Window(_) => "a window",
        response::Payload::Made(_) => "a pane",
        response::Payload::WindowFrame(_) => "a window frame",
        response::Payload::PaneText(_) => "a pane's text",
        response::Payload::Daemons(_) => "a list of daemons",
        response::Payload::PaneState(_) => "a pane's state",
        response::Payload::PaneClosed(_) => "a closed pane",
        response::Payload::Unanswered(_) => "a request nobody answered",
        response::Payload::BackendHealth(_) => "a daemon's health",
        response::Payload::KeyHandled(_) => "a keystroke's outcome",
        response::Payload::Asking(_) => "the pane that asked",
        response::Payload::Went(_) => "where the focus history went",
        response::Payload::Opened(_) => "a window opened",
        response::Payload::Reopening(_) => "the windows to reopen",
        response::Payload::AppClaim(_) => "a claim on the app",
        response::Payload::LayoutMoved(_) => "word that the layout may have moved",
    }
}

/// Every window under this home, one row each: the open ones, or with `closed` the closed ones.
///
/// A summary rather than each window's whole answer: what somebody running this wants is which
/// window is which, and the name to reach it by. Every window of an app answers on one socket, so
/// each answer is the window that answered and the others it describes. More than one answer is
/// more than one app - two installs, or a development build beside the release - and each row
/// then says which socket reaches it.
pub fn windows(
    answers: &[(String, Result<Response, Trouble>)],
    here: Option<&str>,
    closed: bool,
    json: bool,
) -> String {
    let rows = window_rows(answers, here, closed);
    if json {
        let listed: Vec<Value> = rows
            .iter()
            .map(|row| match &row.unreadable {
                // Listed with its reason rather than dropped: an app that is there and will not
                // answer is the case somebody running this is looking for.
                Some(detail) => {
                    json!({ "socket": row.socket, "here": false, "unreadable": detail })
                }
                None => json!({
                    "window": row.name,
                    "open": !closed,
                    "here": row.here,
                    "socket": row.socket,
                    "panes": row.panes,
                    "tabs": row.tabs,
                    "keyboard": row.keyboard,
                }),
            })
            .collect();
        return json!({ "windows": listed }).to_string();
    }

    if answers.is_empty() {
        return styled("no Muster window is listening", QUIET);
    }
    if rows.is_empty() {
        return styled(if closed { "no window is closed" } else { "no window is open" }, QUIET);
    }
    let apps = answers.len() > 1;
    let width = rows.iter().map(|row| row.name.chars().count()).max().unwrap_or(0);
    rows.iter()
        .map(|row| {
            if let Some(detail) = &row.unreadable {
                return format!("  {} {}", styled(&row.socket, NAME), styled(detail, QUIET));
            }
            let mark = if row.here { "▸" } else { " " };
            let mut line = format!(
                "{mark} {}  {}",
                styled(&format!("{:width$}", row.name), NAME),
                styled(&format!("{} panes, {} tabs", row.panes, row.tabs), QUIET)
            );
            if apps {
                line.push_str("  ");
                line.push_str(&styled(&row.socket, QUIET));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One row of `muster window list`.
struct WindowRow {
    name: String,
    socket: String,
    here: bool,
    panes: usize,
    tabs: usize,
    /// The pane the keyboard is on, which only the window that answered can say.
    keyboard: Option<String>,
    unreadable: Option<String>,
}

fn window_rows(
    answers: &[(String, Result<Response, Trouble>)],
    here: Option<&str>,
    closed: bool,
) -> Vec<WindowRow> {
    let mut rows = Vec::new();
    for (socket, answer) in answers {
        let Ok(Some(response::Payload::Window(window))) =
            answer.as_ref().map(|response| response.payload.as_ref())
        else {
            let detail = match answer {
                Err(trouble) => trouble.detail().to_string(),
                Ok(_) => "the app answered with something else".to_string(),
            };
            rows.push(WindowRow {
                name: String::new(),
                socket: socket.clone(),
                here: false,
                panes: 0,
                tabs: 0,
                keyboard: None,
                unreadable: Some(detail),
            });
            continue;
        };
        if !closed {
            rows.push(WindowRow {
                name: window.name.clone(),
                socket: socket.clone(),
                // The window that answered is the one a command here would reach: the window
                // holding this pane's tab, from inside one.
                here: here == Some(socket.as_str()),
                panes: counted_panes(window),
                tabs: counted_tabs(window),
                keyboard: keyboard_pane(window),
                unreadable: None,
            });
        }
        for other in window.windows.iter().filter(|other| (other.pid == 0) == closed) {
            rows.push(WindowRow {
                name: other.name.clone(),
                socket: socket.clone(),
                here: false,
                panes: other.tabs.iter().map(|tab| tab.panes.len()).sum(),
                tabs: other.tabs.len(),
                keyboard: None,
                unreadable: None,
            });
        }
    }
    rows
}

/// Every app's whole answer, one after another, when the caller narrowed to none of them.
///
/// Distinct from [`windows`] above, which is `window list` and is a summary: this is what
/// `muster window` prints, asked of each app that answered. Every window of an app answers on one
/// socket, so more than one answer is more than one app - two installs, or two homes - and each
/// answer covers all of that app's windows. Headed by the window that answered, with the socket
/// beside it in both shapes, since that is what `--socket` takes and what the next command needs.
pub fn answers(answers: &[(String, Result<Response, Trouble>)], json: bool) -> String {
    if json {
        let listed: Vec<Value> = answers
            .iter()
            .map(|(path, answer)| {
                let mut row = json!({ "socket": path });
                match answer {
                    Ok(response) => match &response.payload {
                        Some(response::Payload::Window(window)) => {
                            // Flattened into the row rather than nested under a key, so that one
                            // window's answer is the same object here as it is on its own and a
                            // filter written for one reads across all of them:
                            // `.windows[].panes[] | select(.state == "blocked")`.
                            if let Value::Object(fields) = window_json(window, Others::All) {
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
            let name = match answer {
                Ok(Response { payload: Some(response::Payload::Window(window)) })
                    if !window.name.is_empty() =>
                {
                    window.name.clone()
                }
                _ => "a Muster".to_string(),
            };
            let heading = format!("{} {}", styled(&name, NAME), styled(path, QUIET));
            let body = match answer {
                Ok(response) => match &response.payload {
                    Some(response::Payload::Window(window)) => {
                        window_text(window, now_ms(), Others::All, true)
                    }
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
/// Which other windows a window's answer lists: all of them, or none for an answer that has no
/// windows to speak of, such as the daemon's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Others {
    All,
    None,
}

impl Others {
    fn include(self, _other: &muster_proto::OtherWindow) -> bool {
        self == Others::All
    }
}

/// How another window is headed: by its name, which is what `muster tab move --window` and
/// `--window` take, and whether it is closed.
fn other_heading(other: &muster_proto::OtherWindow) -> String {
    if other.pid == 0 { format!("{} (closed)", other.name) } else { other.name.clone() }
}

fn window_text(window: &Window, now_ms: i64, others: Others, drawn: bool) -> String {
    let keyboard = keyboard_pane(window);
    let states = states(window);
    let widths = Widths::across(window, &states, now_ms);
    let machines: Vec<&muster_proto::Machine> = window.daemons.iter().collect();
    let say_machine = machines.len() > 1;

    let mut lines: Vec<String> = Vec::new();
    let drawing = Drawing::of(window, keyboard.as_deref(), say_machine);
    for tab in tabs(window) {
        match drawing.as_ref().and_then(|drawing| drawing.layout(&tab.tab_id)) {
            Some(layout) => {
                lines.push(drawn_tab_line(&widths, tab, layout));
                if let Some(drawing) = &drawing {
                    lines.extend(drawing.tab(layout, tab, &states, terminal_width()));
                }
            }
            None => lines.extend(tab_lines(
                &widths,
                tab,
                &states,
                keyboard.as_deref(),
                now_ms,
                say_machine,
                drawn,
            )),
        }
    }

    // Other windows' tabs, after this window's own. A tab belongs to exactly one window, so
    // these are not this window's to show - but any verb works from any window, and a closed
    // window's agents are still running with nobody else to list them.
    for other in window.windows.iter().filter(|other| others.include(other)) {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(styled(&other_heading(other), NAME));
        for tab in &other.tabs {
            // Drawn when the answer laid it out, which a window does for every window's tabs; a
            // window too old to lay out another window's tabs has them listed.
            match drawing.as_ref().zip(drawing.as_ref().and_then(|d| d.layout(&tab.tab_id))) {
                Some((drawing, layout)) => {
                    lines.push(drawn_tab_line(&widths, tab, layout));
                    lines.extend(drawing.tab(layout, tab, &states, terminal_width()));
                }
                None => {
                    lines.extend(tab_lines(
                        &widths,
                        tab,
                        &states,
                        None,
                        now_ms,
                        say_machine,
                        drawn,
                    ));
                }
            }
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

/// A drawn tab's heading: its row, and which pane it is zoomed on when it is.
fn drawn_tab_line(
    widths: &Widths,
    tab: &muster_proto::RosterTab,
    layout: &muster_proto::TabLayout,
) -> String {
    let mut heading = tab_line(widths, tab);
    if let Some(zoomed) = layout.regions.iter().find(|region| region.zoomed) {
        heading.push_str("  ");
        heading.push_str(&styled(&format!("zoomed on {}", zoomed.pane_id), QUIET));
    }
    heading
}

/// What `--layout` draws from: every tab's arrangement and every pane's size, from an answer
/// that asked for them. `None` from one that did not, which is drawn as the list it always was.
struct Drawing<'a> {
    layouts: BTreeMap<&'a str, &'a muster_proto::TabLayout>,
    grids: BTreeMap<&'a str, &'a muster_proto::PaneGrid>,
    keyboard: Option<&'a str>,
    say_machine: bool,
}

impl<'a> Drawing<'a> {
    fn of(window: &'a Window, keyboard: Option<&'a str>, say_machine: bool) -> Option<Drawing<'a>> {
        if window.layouts.is_empty() {
            return None;
        }
        Some(Drawing {
            layouts: window.layouts.iter().map(|layout| (layout.tab_id.as_str(), layout)).collect(),
            grids: grids(window),
            keyboard,
            say_machine,
        })
    }

    fn layout(&self, tab: &str) -> Option<&'a muster_proto::TabLayout> {
        self.layouts.get(tab).copied()
    }

    /// One tab's boxes, `width` columns wide.
    fn tab(
        &self,
        layout: &muster_proto::TabLayout,
        tab: &muster_proto::RosterTab,
        states: &BTreeMap<&str, &muster_proto::PaneStateChanged>,
        width: usize,
    ) -> Vec<String> {
        let mut order: Vec<&str> = Vec::new();
        // A zoomed part is drawn as the one pane filling it, which is what is on screen and what
        // its daemon sizes that pane to. The panes behind it keep the sizes they had before the
        // zoom, and drawn in the tree beside a pane sized to the whole tab their boxes and their
        // sizes would disagree, so they are named under the drawing instead.
        let mut behind: Vec<&str> = Vec::new();
        let parts: Vec<Part> = layout
            .regions
            .iter()
            .map(|region| {
                let root = match region.root.as_ref() {
                    Some(root) if region.zoomed && !region.pane_id.is_empty() => {
                        let mut all = Vec::new();
                        tree(root, &mut all);
                        behind.extend(all.into_iter().filter(|pane| *pane != region.pane_id));
                        order.push(region.pane_id.as_str());
                        Some(diagram::Node::Pane(order.len() - 1))
                    }
                    Some(root) => tree(root, &mut order),
                    None => None,
                };
                Part { weight: region.weight, root }
            })
            .collect();
        let shares: BTreeMap<&str, &muster_proto::PanePlace> =
            layout.places.iter().map(|place| (place.pane_id.as_str(), place)).collect();
        let boxes: Vec<Lines> = order
            .iter()
            .map(|pane| {
                let row = tab.panes.iter().find(|row| row.pane_id == *pane);
                let agent = states.get(pane).copied();
                let size = match (self.grids.get(pane), shares.get(pane)) {
                    (Some(grid), _) => format!("{}x{}", grid.cols, grid.rows),
                    (None, Some(share)) => format!(
                        "{}% x {}%",
                        (share.width * 100.0).round(),
                        (share.height * 100.0).round()
                    ),
                    (None, None) => String::new(),
                };
                pane_box(pane, row, agent, &size, self.keyboard == Some(*pane), self.say_machine)
            })
            .collect();
        let mut lines = diagram::draw(&parts, &boxes, width);
        if !behind.is_empty() {
            lines.push(styled(&format!("behind the zoom: {}", behind.join(", ")), QUIET));
        }
        lines
    }
}

/// What one pane's box says: its name, its label, what its agent is doing and how big it is, and
/// which machine it is on when more than one is attached.
fn pane_box(
    pane: &str,
    row: Option<&muster_proto::RosterPane>,
    agent: Option<&muster_proto::PaneStateChanged>,
    size: &str,
    keyboard: bool,
    say_machine: bool,
) -> Lines {
    let mut name = Vec::new();
    if keyboard {
        name.push(("▸ ".to_string(), NAME));
    }
    name.push((pane.to_string(), NAME));
    let mut lines = vec![name];
    if let Some(row) = row.filter(|row| !row.label.is_empty()) {
        lines.push(vec![(row.label.clone(), PLAIN)]);
    }
    let state = agent.map_or("unknown", |agent| agent.state.as_str());
    let mut doing = vec![(state.to_string(), agent_style(state))];
    if !size.is_empty() {
        doing.push((format!(" · {size}"), PLAIN));
    }
    lines.push(doing);
    if say_machine && let Some(row) = row {
        lines.push(vec![(format!("on {}", row.daemon_id), QUIET)]);
    }
    lines
}

/// A region's tree as the diagram draws it, naming its panes in `order` as they are met.
fn tree<'a>(node: &'a muster_proto::ViewNode, order: &mut Vec<&'a str>) -> Option<diagram::Node> {
    match node.node.as_ref()? {
        muster_proto::view_node::Node::Pane(pane) => {
            order.push(pane.pane_id.as_str());
            Some(diagram::Node::Pane(order.len() - 1))
        }
        muster_proto::view_node::Node::Split(split) => Some(diagram::Node::Split {
            columns: split.axis != "rows",
            ratio: split.ratio,
            first: Box::new(tree(split.first.as_deref()?, order)?),
            second: Box::new(tree(split.second.as_deref()?, order)?),
        }),
    }
}

/// How wide to draw: the terminal's width when this is one, `COLUMNS` when that says, and 80
/// otherwise - kept between 40, below which no box holds a name, and 160, past which a diagram
/// is harder to read than it is accurate.
fn terminal_width() -> usize {
    let columns = terminal_columns()
        .or_else(|| std::env::var("COLUMNS").ok().and_then(|columns| columns.parse().ok()))
        .unwrap_or(80);
    columns.clamp(40, 160)
}

fn terminal_columns() -> Option<usize> {
    let mut size = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCGWINSZ writes one winsize into the struct handed to it and reads nothing else.
    let read = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &raw mut size) };
    (read == 0 && size.ws_col > 0).then_some(usize::from(size.ws_col))
}

/// Each pane's size in cells, by pane, from an answer that asked for the layout.
fn grids(window: &Window) -> BTreeMap<&str, &muster_proto::PaneGrid> {
    window.grids.iter().map(|grid| (grid.pane_id.as_str(), grid)).collect()
}

/// A tab's line, and a line for each pane in it.
fn tab_lines(
    widths: &Widths,
    tab: &muster_proto::RosterTab,
    states: &BTreeMap<&str, &muster_proto::PaneStateChanged>,
    keyboard: Option<&str>,
    now_ms: i64,
    say_machine: bool,
    drawn: bool,
) -> Vec<String> {
    let mut lines = vec![tab_line(widths, tab)];
    for pane in &tab.panes {
        let agent = states.get(pane.pane_id.as_str()).copied();
        lines.push(pane_line(
            widths,
            pane,
            agent,
            now_ms,
            keyboard == Some(pane.pane_id.as_str()),
            say_machine,
            drawn,
        ));
    }
    lines
}

/// A machine's heading, and beneath it what would end with the daemon behind it.
///
/// Two lines rather than one, and the second is the one this was missing. A person deciding
/// whether a herdr process was safe to end, when Muster ran herdr, could not pair a pid with the
/// work it held, so the choice was made on age, and age picks the wrong process (kan
/// a_28YghIUw2). Muster started the daemon or chose to attach to it, so it can simply say.
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
        place(tab.place),
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

/// A place as a row prints it: blank for another window's tab or pane, which is that window's to
/// number, so a person does not press a chord here for a number that means something there.
fn place(place: u32) -> String {
    if place == 0 { " ".to_string() } else { place.to_string() }
}

/// One pane's row: which one has the keyboard in the gutter, then place, name, state, how long it
/// has been in that state, and label.
///
/// The keyboard is marked in the gutter rather than in a column of its own, the way any list marks
/// the item you are on - a column would put eight blank spaces on every other row. Being off screen
/// is a note after the label instead, because most panes are on screen and the exception is the
/// thing worth saying.
fn pane_line(
    widths: &Widths,
    pane: &muster_proto::RosterPane,
    agent: Option<&muster_proto::PaneStateChanged>,
    now_ms: i64,
    has_keyboard: bool,
    say_machine: bool,
    drawn: bool,
) -> String {
    let state = agent.map_or("unknown", |agent| agent.state.as_str());
    let held = agent.map(|agent| held_for(agent.since_ms, now_ms)).unwrap_or_default();
    let mut line = format!(
        "  {}{}  {}  {}  {}  {}",
        if has_keyboard { styled("▸", NAME) + " " } else { "  ".to_string() },
        pad(&place(pane.place), widths.place, QUIET, Align::Right),
        pad(&pane.pane_id, widths.name, PLAIN, Align::Left),
        pad(state, widths.state, agent_style(state), Align::Left),
        pad(&held, widths.held, QUIET, Align::Right),
        pane.label,
    );
    let waiting = agent.and_then(|agent| agent.facts.as_ref()).map(|facts| facts.waiting.as_str());
    // What it waits on says more than its title while it waits, and the two would crowd a line.
    match waiting.filter(|waiting| state == "waiting" && !waiting.is_empty()) {
        Some(waiting) => line.push_str(&styled(&format!(" · on {waiting}"), QUIET)),
        None if !pane.subtitle.is_empty() => {
            line.push_str(&styled(&format!(" · {}", pane.subtitle), QUIET));
        }
        None => {}
    }
    if let Some(agent) = agent {
        for said in agent_notes(agent) {
            line.push_str(&styled(&format!("  {said}"), QUIET));
        }
    }
    // Only a window draws panes, so with none there is nothing to be hidden from.
    if drawn && !pane.on_screen {
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

/// What else is worth a glance about a pane's agent, each a few words: how full its context is,
/// its sub-agents, whether its screen can be read, a bell nobody heard, and a program's progress.
///
/// The model and the cost are left to `--json`: they change what somebody does next less often
/// than they would lengthen every line.
fn agent_notes(agent: &muster_proto::PaneStateChanged) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(facts) = &agent.facts {
        if let Some(used) = facts.context_used {
            notes.push(format!("{used:.0}% context"));
        }
        match facts.subagents {
            0 => {}
            1 => notes.push("1 sub-agent".to_string()),
            count => notes.push(format!("{count} sub-agents")),
        }
    }
    if agent.unreadable {
        notes.push("(cannot read the screen)".to_string());
    }
    if agent.rang {
        notes.push("(bell)".to_string());
    }
    if let Some(progress) = &agent.progress {
        match (progress.state.as_str(), progress.percent) {
            ("error", _) => notes.push("(progress failed)".to_string()),
            ("running" | "paused", Some(percent)) => notes.push(format!("({percent}% done)")),
            _ => {}
        }
    }
    notes
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
/// Waiting is blue, the nearest of the sixteen to the window's indigo. Blue's legibility cost
/// matters less here than it did for working: a waiting agent needs nobody, so its row is one
/// to skip rather than to find.
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
        "waiting" => Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Blue))),
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
    held: usize,
}

impl Widths {
    fn across(
        window: &Window,
        states: &BTreeMap<&str, &muster_proto::PaneStateChanged>,
        now_ms: i64,
    ) -> Widths {
        let mut widths = Widths { place: 1, name: 0, state: 0, held: 0 };
        for tab in tabs(window) {
            widths.name = widths.name.max(tab.tab_id.width());
            for pane in &tab.panes {
                widths.place = widths.place.max(pane.place.to_string().width());
                widths.name = widths.name.max(pane.pane_id.width());
                let agent = states.get(pane.pane_id.as_str());
                let state = agent.map_or("unknown", |agent| agent.state.as_str());
                widths.state = widths.state.max(state.width());
                let held = agent.map(|agent| held_for(agent.since_ms, now_ms)).unwrap_or_default();
                widths.held = widths.held.max(held.width());
            }
        }
        widths
    }
}

/// What an agent says about itself, with what it has not said as null rather than as zero: an
/// agent that never reported its context has not used none of it.
fn facts_json(facts: &muster_proto::AgentFacts) -> Value {
    let said = |text: &str| if text.is_empty() { Value::Null } else { json!(text) };
    json!({
        "context_used": facts.context_used,
        "subagents": facts.subagents,
        "model": said(&facts.model),
        "cost_usd": facts.cost_usd,
        "waiting": said(&facts.waiting),
        "other": facts.other,
    })
}

/// How long something has held since `since_ms`, in the one largest unit that fits: `45s`,
/// `12m`, `3h`, `2d`.
///
/// One unit, because a row read at a glance wants "blocked 40m" and nothing finer - somebody
/// deciding which agent has waited longest is comparing orders of magnitude. Empty for zero,
/// which is a window too old to say, and a clock that has gone backwards reads as `0s` rather
/// than as a negative.
fn held_for(since_ms: i64, now_ms: i64) -> String {
    if since_ms == 0 {
        return String::new();
    }
    let seconds = (now_ms - since_ms).max(0) / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Milliseconds since the epoch, read once per answer so every row is measured from one moment.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// Seconds since the epoch, to the millisecond, or null for a window too old to say.
///
/// Seconds, like `started` in `muster daemons`, so `now - .since` in jq is how long a pane has
/// been in its state. The fraction keeps two changes inside one second from reading as one.
#[allow(clippy::cast_precision_loss)]
fn since_json(since_ms: i64) -> Value {
    if since_ms == 0 { Value::Null } else { json!(since_ms as f64 / 1000.0) }
}

/// A window as a program reads it: one flat list of panes, each carrying where it sits.
///
/// Flat because that is what filtering wants - "every blocked pane" is one pass here and a nested
/// walk on the wire shape - and each pane names its daemon and its tab so nothing is lost by it.
fn window_json(window: &Window, others: Others) -> Value {
    let keyboard = keyboard_pane(window);
    let states = states(window);
    let places = places(window);

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
                "state": states.get(pane.pane_id.as_str()).map_or("unknown", |agent| agent.state.as_str()),
                "since": states.get(pane.pane_id.as_str()).map_or(Value::Null, |agent| since_json(agent.since_ms)),
                "reported": states.get(pane.pane_id.as_str()).is_some_and(|agent| agent.reported),
                "unreadable": states.get(pane.pane_id.as_str()).is_some_and(|agent| agent.unreadable),
                "facts": states.get(pane.pane_id.as_str()).and_then(|agent| agent.facts.as_ref()).map_or(Value::Null, facts_json),
                "progress": states.get(pane.pane_id.as_str()).and_then(|agent| agent.progress.as_ref()).map_or(Value::Null, |progress| {
                    json!({ "state": progress.state, "percent": progress.percent })
                }),
                "rang": states.get(pane.pane_id.as_str()).is_some_and(|agent| agent.rang),
                "on_screen": pane.on_screen,
                "keyboard": keyboard.as_deref() == Some(pane.pane_id.as_str()),
                // Null rather than zeroes for a pane the window is not drawing, so this and
                // `on_screen` above cannot say different things: a rectangle of no size is a
                // place, and a pane behind a zoom or in a background tab has none.
                "rect": places.get(pane.pane_id.as_str()).copied().map_or(Value::Null, rect_json),
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
    // Named so as not to read as the list `{"windows": [...]}` a caller outside any pane gets:
    // these are the windows other than this one, as this one knows them.
    let mut other_windows: Vec<Value> = window
        .windows
        .iter()
        .filter(|other| others.include(other))
        .map(|other| {
            json!({
                "window": other.name,
                "pid": if other.pid == 0 { Value::Null } else { json!(other.pid) },
                "tabs": other.tabs.iter().map(|tab| json!({
                    "tab": tab.tab_id,
                    "daemons": tab.daemon_ids,
                    "label": tab.label,
                    "given_name": tab.given_name,
                    "panes": tab.panes.iter().map(|pane| json!({
                        "pane": pane.pane_id,
                        "daemon": pane.daemon_id,
                        "label": pane.label,
                        "state": states.get(pane.pane_id.as_str())
                            .map_or("unknown", |agent| agent.state.as_str()),
                    })).collect::<Vec<Value>>(),
                })).collect::<Vec<Value>>(),
            })
        })
        .collect();
    add_layout(window, &mut tabs_out, &mut panes, &mut other_windows);

    json!({
        "name": window.name,
        "other_windows": other_windows,
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

fn rect_json(place: &muster_proto::PanePlace) -> Value {
    json!({ "x": place.x, "y": place.y, "width": place.width, "height": place.height })
}

/// What `--layout` adds to `tabs[]` and `panes[]`: each tab's regions, and each pane's frame and
/// size. Only when the layout was asked for, and absent rather than null otherwise, so a size
/// nobody asked for is never read as a size nobody knows.
///
/// The other windows' panes get a frame and a size too, a closed window's included. A frame is a
/// place in the pane's own tab, so it means the same whichever window holds that tab.
fn add_layout(window: &Window, tabs: &mut [Value], panes: &mut [Value], others: &mut [Value]) {
    if window.layouts.is_empty() {
        return;
    }
    let layouts: BTreeMap<&str, &muster_proto::TabLayout> =
        window.layouts.iter().map(|layout| (layout.tab_id.as_str(), layout)).collect();
    let frames: BTreeMap<&str, &muster_proto::PanePlace> = window
        .layouts
        .iter()
        .flat_map(|layout| &layout.places)
        .map(|place| (place.pane_id.as_str(), place))
        .collect();
    let grids = grids(window);
    let size = |pane: &mut Value| {
        let Value::Object(fields) = pane else { return };
        let name = fields.get("pane").and_then(Value::as_str).unwrap_or_default().to_string();
        // Where it sits in its tab's arrangement, shown or not, in `rect`'s terms.
        let frame = frames.get(name.as_str()).copied().map_or(Value::Null, rect_json);
        // Null when its daemon could not say, which is not the same as no size.
        let cells = grids
            .get(name.as_str())
            .map_or(Value::Null, |grid| json!({ "cols": grid.cols, "rows": grid.rows }));
        fields.insert("frame".to_string(), frame);
        fields.insert("cells".to_string(), cells);
    };
    for tab in tabs.iter_mut() {
        let Value::Object(fields) = tab else { continue };
        let regions = fields
            .get("tab")
            .and_then(Value::as_str)
            .and_then(|name| layouts.get(name))
            .map(|layout| regions_json(&layout.regions, &layout.focused_region))
            .unwrap_or_default();
        fields.insert("regions".to_string(), Value::Array(regions));
    }
    panes.iter_mut().for_each(size);
    for other in others.iter_mut() {
        for tab in other["tabs"].as_array_mut().into_iter().flatten() {
            tab["panes"].as_array_mut().into_iter().flatten().for_each(size);
        }
    }
}

/// Where each pane on screen sits, by pane.
///
/// Keyed by pane alone, like [`states`] and for the same reason: a pane name is Muster's own and
/// unique across every machine the window is showing.
fn places(window: &Window) -> BTreeMap<&str, &muster_proto::PanePlace> {
    window.places.iter().map(|place| (place.pane_id.as_str(), place)).collect()
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
    regions_json(&view.regions, &view.focused_region)
}

/// Regions as `regions[]` spells them, for the tab on screen and, under `--layout`, every tab.
fn regions_json(regions: &[muster_proto::ViewRegion], focused: &str) -> Vec<Value> {
    regions
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
                "keyboard": !focused.is_empty() && region.region_id == focused,
                // Whether this region is filled by one pane rather than by its tab's whole
                // tree. Reported because it is the difference between a tab holding one pane
                // and a tab whose others are hidden, and nothing else in this answer implies
                // it: every pane in the tab still lists, and the ones a zoom is covering read
                // as `on_screen: false` exactly like the panes of a tab in the background.
                "zoomed": region.zoomed,
                // How this part splits, which is the only place the answer says so. `rect` on
                // each pane says where everything ended up; this says the shape that put it
                // there, and which divider a caller would be moving. Null while the daemon has
                // not said how the tab is arranged - an ordinary moment, not a failure, and a
                // different answer from a part holding no panes.
                //
                // Resolved for a zoom on screen, like `zoomed` beside it: a zoomed part is one
                // pane there, because that is what is drawn. Under `tabs[].regions` it is the
                // whole tree, because that says how the tab is laid out.
                "layout": region.root.as_ref().map_or(Value::Null, layout_json),
            })
        })
        .collect()
}

/// One node of a part's tree, and everything under it.
fn layout_json(node: &muster_proto::ViewNode) -> Value {
    match node.node.as_ref() {
        Some(muster_proto::view_node::Node::Pane(pane)) => json!({ "pane": pane.pane_id }),
        Some(muster_proto::view_node::Node::Split(split)) => json!({
            // `columns` puts its children side by side and `rows` stacks them, so a row of
            // panes is a `columns` split. Muster's own spelling rather than a backend's, which
            // says which way a split was made rather than how to lay two children out.
            "axis": split.axis,
            "ratio": split.ratio,
            "first": split.first.as_deref().map_or(Value::Null, layout_json),
            "second": split.second.as_deref().map_or(Value::Null, layout_json),
        }),
        // A node the app sent with neither half set. Nothing builds one, so this is a schema
        // this CLI is too old to read - said rather than silently rendered as a leaf.
        None => Value::Null,
    }
}

/// Which pane this window's keyboard is on, by way of the region that has it.
fn keyboard_pane(window: &Window) -> Option<String> {
    let view = window.view.as_ref()?;
    let region = view.regions.iter().find(|region| region.region_id == view.focused_region)?;
    Some(region.pane_id.clone()).filter(|pane| !pane.is_empty())
}

/// What each pane's agent is doing and since when, by pane.
///
/// Keyed by pane alone even though the messages carry a daemon too: a pane name is Muster's own and
/// unique across every machine in the window, which is the whole reason a caller can address one
/// without knowing where it lives.
fn states(window: &Window) -> BTreeMap<&str, &muster_proto::PaneStateChanged> {
    window.panes.iter().map(|pane| (pane.pane_id.as_str(), pane)).collect()
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
        // A herdr daemon holds no lock file of muster-daemon's, so the line below does not
        // reach it; its listener is the process to end.
        if daemon.state == "herdr" {
            lines.push(format!(
                "  {}End it, and every pane it holds, with: kill $(lsof -t {}){}",
                QUIET.render(),
                shell_word(&daemon.socket),
                QUIET.render_reset()
            ));
        }
    }
    lines.push(String::new());
    lines.push(format!(
        "{}End one with: kill $(lsof -t <socket>.lock){}",
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
        "herdr" => "herdr · started by a Muster from before it had a daemon of its own, and \
                    still running: its panes go on, and no window of this Muster can show them"
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

/// `text` as one word a POSIX shell reads back unchanged, for a command printed to be pasted:
/// as it is when it needs no quoting, which is every path Muster makes, and single-quoted
/// otherwise.
fn shell_word(text: &str) -> String {
    let plain = !text.is_empty()
        && text.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@%,=".contains(c));
    if plain { text.to_string() } else { format!("'{}'", text.replace('\'', "'\\''")) }
}

#[cfg(test)]
mod tests {
    use super::{Others, QUIET, agent_style, daemons_text, held_for, window_json, window_text};
    use anstyle::{AnsiColor, Color, Style};

    fn hue(color: AnsiColor) -> Style {
        Style::new().fg_color(Some(Color::Ansi(color)))
    }

    /// A herdr daemon an older Muster left running is named for what it is, with the way to end
    /// it: muster-daemon's `.lock` line cannot reach it, and nothing else would say what it holds.
    #[test]
    fn a_herdr_daemon_from_before_is_named_with_how_to_end_it() {
        let daemon = |socket: &str, state: &str| muster_proto::KnownDaemon {
            socket: socket.to_string(),
            state: state.to_string(),
            ..muster_proto::KnownDaemon::default()
        };
        let text = daemons_text(&muster_proto::Daemons {
            remembered: true,
            daemons: vec![
                daemon("/home/a/.config/herdr/sessions/muster/herdr.sock", "herdr"),
                daemon("/home/a/.muster/daemon/i1.sock", "answering"),
            ],
        });
        assert!(text.contains("herdr · started by a Muster from before"), "{text}");
        assert!(
            text.contains("kill $(lsof -t /home/a/.config/herdr/sessions/muster/herdr.sock)"),
            "{text}"
        );
        assert!(!text.contains("lsof -t /home/a/.muster/daemon/i1.sock)"), "{text}");
    }

    /// The line is there to be pasted, and a home directory can have a space in it.
    #[test]
    fn the_way_to_end_a_herdr_daemon_pastes_whatever_its_path() {
        let text = daemons_text(&muster_proto::Daemons {
            remembered: true,
            daemons: vec![muster_proto::KnownDaemon {
                socket: "/Users/Jo Smith/.config/herdr/herdr.sock".to_string(),
                state: "herdr".to_string(),
                ..muster_proto::KnownDaemon::default()
            }],
        });
        assert!(
            text.contains("kill $(lsof -t '/Users/Jo Smith/.config/herdr/herdr.sock')"),
            "{text}"
        );
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

    /// Under `--layout`, a pane in another window, open or closed, has a frame in its own tab like
    /// a pane here, and one in a tab the answer did not lay out has none.
    #[test]
    fn every_windows_panes_are_laid_out() {
        use muster_proto::{
            OtherWindow, PanePlace, RosterChanged, RosterPane, RosterTab, TabLayout, Window,
        };
        let tab = |tab: &str, pane: &str| RosterTab {
            tab_id: tab.to_string(),
            panes: vec![RosterPane { pane_id: pane.to_string(), ..RosterPane::default() }],
            ..RosterTab::default()
        };
        let laid_out = |tab: &str, pane: &str| TabLayout {
            tab_id: tab.to_string(),
            places: vec![PanePlace {
                pane_id: pane.to_string(),
                width: 1.0,
                height: 1.0,
                ..PanePlace::default()
            }],
            ..TabLayout::default()
        };
        let window = Window {
            name: "window-1".to_string(),
            roster: Some(RosterChanged { tabs: vec![tab("t1", "p1")], ..RosterChanged::default() }),
            windows: vec![
                OtherWindow { name: "window-2".to_string(), pid: 7, tabs: vec![tab("t2", "p2")] },
                OtherWindow { name: "window-3".to_string(), pid: 0, tabs: vec![tab("t3", "p3")] },
            ],
            layouts: vec![laid_out("t1", "p1"), laid_out("t3", "p3")],
            ..Window::default()
        };
        let json = window_json(&window, Others::All);
        assert_eq!(json["panes"][0]["frame"]["width"], 1.0, "{json}");
        let closed = &json["other_windows"][1]["tabs"][0]["panes"][0];
        assert_eq!(closed["frame"]["height"], 1.0, "a closed window's pane: {json}");
        let unlaid = &json["other_windows"][0]["tabs"][0]["panes"][0];
        assert!(unlaid["frame"].is_null(), "a pane in a tab nothing laid out: {json}");
    }

    /// A zoomed tab is drawn as the pane filling it, at the size its daemon gives it, and the
    /// panes behind the zoom are named rather than drawn at sizes from before it.
    #[test]
    fn a_zoomed_tab_is_drawn_as_the_pane_that_fills_it() {
        use muster_proto::{
            PaneGrid, RosterChanged, RosterPane, RosterTab, TabLayout, ViewNode, ViewPane,
            ViewRegion, ViewSplit, Window, view_node,
        };
        let leaf = |pane: &str| ViewNode {
            node: Some(view_node::Node::Pane(ViewPane {
                pane_id: pane.to_string(),
                ..ViewPane::default()
            })),
        };
        let grid = |pane: &str, cols: u32| PaneGrid {
            pane_id: pane.to_string(),
            cols,
            rows: 40,
            ..PaneGrid::default()
        };
        let window = Window {
            roster: Some(RosterChanged {
                tabs: vec![RosterTab {
                    tab_id: "t1".to_string(),
                    panes: ["p1", "p2"]
                        .map(|pane| RosterPane {
                            pane_id: pane.to_string(),
                            ..RosterPane::default()
                        })
                        .to_vec(),
                    ..RosterTab::default()
                }],
                ..RosterChanged::default()
            }),
            layouts: vec![TabLayout {
                tab_id: "t1".to_string(),
                regions: vec![ViewRegion {
                    pane_id: "p1".to_string(),
                    zoomed: true,
                    weight: 1.0,
                    root: Some(ViewNode {
                        node: Some(view_node::Node::Split(Box::new(ViewSplit {
                            axis: "columns".to_string(),
                            ratio: 0.5,
                            first: Some(Box::new(leaf("p1"))),
                            second: Some(Box::new(leaf("p2"))),
                        }))),
                    }),
                    ..ViewRegion::default()
                }],
                ..TabLayout::default()
            }],
            grids: vec![grid("p1", 160), grid("p2", 79)],
            ..Window::default()
        };
        let text = window_text(&window, 0, Others::All, false);
        assert!(text.contains("160x40"), "the zoomed pane is drawn at its size: {text}");
        assert!(!text.contains("79x40"), "a pane behind the zoom is drawn: {text}");
        assert!(text.contains("behind the zoom: p2"), "{text}");
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

    /// One unit per row, the largest that fits, so a glance down the column compares orders of
    /// magnitude. The boundaries are where a row changes unit.
    #[test]
    fn a_duration_reads_in_its_largest_whole_unit() {
        let now = 1_757_700_000_000;
        assert_eq!(held_for(now - 59_999, now), "59s");
        assert_eq!(held_for(now - 60_000, now), "1m");
        assert_eq!(held_for(now - 3_599_999, now), "59m");
        assert_eq!(held_for(now - 3_600_000, now), "1h");
        assert_eq!(held_for(now - 86_400_000, now), "1d");
    }

    /// Zero is a window older than the field, which has nothing to say; a stamp from the future
    /// is two clocks disagreeing, and a negative duration would be a wrong thing to print.
    #[test]
    fn a_duration_nobody_stamped_or_from_the_future_says_nothing_wrong() {
        let now = 1_757_700_000_000;
        assert_eq!(held_for(0, now), "");
        assert_eq!(held_for(now + 5_000, now), "0s");
    }
}
