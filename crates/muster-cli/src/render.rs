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
use muster_core::AgentState;
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

/// What was typed, so whoever restarted an agent sees the command it came back with, and can
/// type it again by hand if the agent does not come back.
fn restarted_answer(restarted: &muster_proto::Restarted, json: bool) -> String {
    if json {
        return json!({
            "pane": restarted.pane_id,
            "program": restarted.program,
            "killed": restarted.killed,
            "typed": restarted.typed,
        })
        .to_string();
    }
    let ended = if restarted.killed {
        format!("killed {}, which had not ended when asked to,", restarted.program)
    } else {
        format!("ended {}", restarted.program)
    };
    format!("{}  {ended} and typed: {}", restarted.pane_id, restarted.typed)
}

/// Turns an answer into what should be printed, or into the reason there is nothing to print.
pub fn answer(response: &Response, json: bool) -> Result<String, Trouble> {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => Err(Trouble::Refused(failure.reason.clone())),
        // The same exit as a window that took the request and never answered, one hop further
        // in: the window's daemon took it and never answered, so a retry may repeat it.
        Some(response::Payload::Unanswered(unanswered)) if !unanswered.pane_id.is_empty() => {
            Err(Trouble::MayHaveMade {
                reason: unanswered.reason.clone(),
                pane: unanswered.pane_id.clone(),
            })
        }
        Some(response::Payload::Unanswered(unanswered)) => {
            Err(Trouble::Unanswered(unanswered.reason.clone()))
        }
        Some(response::Payload::Ok(ok)) if !ok.note.is_empty() => Ok(if json {
            json!({ "ok": true, "note": ok.note }).to_string()
        } else {
            ok.note.clone()
        }),
        Some(response::Payload::Ok(_)) => {
            // Nothing to say. Silence is the answer to a request that did what it was asked, and
            // JSON gets an object anyway so that `--json` always produces one.
            Ok(if json { json!({ "ok": true }).to_string() } else { String::new() })
        }
        Some(response::Payload::Made(made)) => Ok(if json {
            let callsign = Some(&made.callsign).filter(|callsign| !callsign.is_empty());
            json!({ "pane": made.pane_id, "callsign": callsign }).to_string()
        } else {
            // The callsign alone, with nothing around it, because the next line of a script, or
            // an agent's next message, is `muster pane send --pane "$(muster pane new --down)"`.
            // The id from a daemon that draws none.
            if made.callsign.is_empty() { made.pane_id.clone() } else { made.callsign.clone() }
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
            let mut value = json!({
                "pane": agent.pane_id,
                "daemon": agent.daemon_id,
                "state": agent.state,
                "since": since_json(agent.since_ms),
            });
            // Left out rather than empty: an empty string is not something to call a pane.
            if !agent.label.is_empty() {
                value["label"] = agent.label.clone().into();
            }
            value.to_string()
        } else {
            // The label last, since it is the one field that can hold a space.
            let state = styled(&agent.state, agent_style(&agent.state));
            format!("{}  {state}  {}", agent.pane_id, agent.label).trim_end().to_string()
        }),
        Some(response::Payload::Restarted(restarted)) => Ok(restarted_answer(restarted, json)),
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
        response::Payload::Restarted(_) => "an agent restarted",
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
                Some(detail) => json!({
                    "window": null,
                    "open": null,
                    "here": false,
                    "socket": row.socket,
                    "unreadable": detail,
                }),
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
                        // Any other answer, such as a pane's text or the daemons, as it would be
                        // answered alone, under `answer`.
                        _ => match answer_alone(response, true) {
                            Ok(text) => {
                                row["answer"] = serde_json::from_str(&text).unwrap_or(json!(text));
                            }
                            Err(detail) => row["unreadable"] = json!(detail),
                        },
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
                    _ => answer_alone(response, false)
                        .unwrap_or_else(|detail| styled(&detail, QUIET)),
                },
                Err(trouble) => styled(trouble.detail(), QUIET),
            };
            format!("{heading}\n{}", body.trim_end())
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// An answer as it is printed when one app gives it, or why it cannot be.
fn answer_alone(response: &Response, json: bool) -> Result<String, String> {
    answer(response, json).map_err(|trouble| trouble.detail().to_string())
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

/// The tabs no window shows, which hiding a pane makes.
fn hidden_tabs(window: &Window) -> impl Iterator<Item = &muster_proto::RosterTab> {
    window.roster.iter().flat_map(|roster| roster.hidden.iter())
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
    let machines: Vec<&muster_proto::Machine> = window.daemons.iter().collect();
    let say_machine = machines.len() > 1;
    let drawing = Drawing::of(window, keyboard.as_deref(), say_machine);
    let helpers = Helpers::of(window, others, drawing.as_ref());
    let widths = Widths::across(window, &states, now_ms, &helpers);
    let rows = Rows { widths: &widths, states: &states, now_ms, say_machine, helpers: &helpers };

    let mut lines: Vec<String> = Vec::new();
    for tab in tabs(window) {
        match drawing.as_ref().and_then(|drawing| drawing.layout(&tab.tab_id)) {
            Some(layout) => {
                lines.push(drawn_tab_line(&widths, tab, layout));
                if let Some(drawing) = &drawing {
                    lines.extend(drawing.tab(layout, tab, &states, terminal_width()));
                }
            }
            None => lines.extend(rows.tab(tab, keyboard.as_deref(), drawn)),
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
                None => lines.extend(rows.tab(tab, None, drawn)),
            }
        }
    }

    // Hidden panes, which no window shows, so no window's list above has them. Listed as a closed
    // window's are, without places or the keyboard, because nothing draws them. A tab whose every
    // pane is listed under its maker above has nothing left to list here.
    let hidden: Vec<&muster_proto::RosterTab> = hidden_tabs(window)
        .filter(|tab| tab.panes.iter().any(|pane| !helpers.moved(pane)))
        .collect();
    if !hidden.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(styled("hidden", NAME));
        for tab in hidden {
            lines.extend(rows.tab(tab, None, false));
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
    let zoomed =
        covering_zoom(layout).or_else(|| layout.regions.iter().find(|region| region.zoomed));
    if let Some(zoomed) = zoomed {
        heading.push_str("  ");
        heading.push_str(&styled(&format!("zoomed on {}", zoomed.pane_id), QUIET));
    }
    heading
}

/// The zoomed part holding the keyboard, which fills the whole tab whatever machines the other
/// parts are on, as the window draws it.
fn covering_zoom(layout: &muster_proto::TabLayout) -> Option<&muster_proto::ViewRegion> {
    layout.regions.iter().find(|region| region.region_id == layout.focused_region && region.zoomed)
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
        // sizes would disagree, so they are named under the drawing instead. While that part
        // holds the keyboard it fills the whole tab, and every other part is behind it too.
        let mut behind: Vec<&str> = Vec::new();
        let covering = covering_zoom(layout);
        let drawn = |region: &&muster_proto::ViewRegion| {
            covering.is_none_or(|covering| covering.region_id == region.region_id)
        };
        for region in layout.regions.iter().filter(|region| !drawn(region)) {
            if let Some(root) = region.root.as_ref() {
                tree(root, &mut behind);
            }
        }
        let parts: Vec<Part> = layout
            .regions
            .iter()
            .filter(drawn)
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

/// What one pane's box says: its callsign, its label, what its agent is doing and how big it is, and
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
    name.push((row.map_or(pane, callsign_or_id).to_string(), NAME));
    let mut lines = vec![name];
    if let Some(beside) = row.map(beside_callsign).filter(|beside| !beside.is_empty()) {
        lines.push(vec![(beside.to_string(), PLAIN)]);
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

/// What every row of a window's listing is drawn against: the columns' widths, what each pane's
/// agent is doing, the time the listing is as of, and whether rows say which machine they are on.
struct Rows<'a> {
    widths: &'a Widths,
    states: &'a BTreeMap<&'a str, &'a muster_proto::PaneStateChanged>,
    now_ms: i64,
    say_machine: bool,
    helpers: &'a Helpers<'a>,
}

impl Rows<'_> {
    /// A tab's row and its panes' rows, `keyboard` marking the pane the keyboard is on.
    fn tab(
        &self,
        tab: &muster_proto::RosterTab,
        keyboard: Option<&str>,
        drawn: bool,
    ) -> Vec<String> {
        let mut lines = vec![tab_line(self.widths, tab)];
        for pane in tab.panes.iter().filter(|pane| !self.helpers.moved(pane)) {
            lines.push(self.pane(pane, keyboard == Some(pane.pane_id.as_str()), drawn, 0));
            self.helper_rows(pane, 1, &mut lines);
        }
        lines
    }

    /// The rows of a pane's hidden helpers, each under its maker one level further in, and theirs
    /// under them.
    fn helper_rows(&self, maker: &muster_proto::RosterPane, depth: usize, lines: &mut Vec<String>) {
        for helper in self.helpers.under(maker) {
            lines.push(self.pane(helper, false, false, depth));
            self.helper_rows(helper, depth + 1, lines);
        }
    }

    /// One pane's row: which one has the keyboard in the gutter, then place, name, state, how long
    /// it has been in that state, and label.
    ///
    /// The keyboard is marked in the gutter rather than in a column of its own, the way any list
    /// marks the item you are on - a column would put eight blank spaces on every other row. Being
    /// off screen is a note after the label instead, because most panes are on screen and the
    /// exception is the thing worth saying.
    ///
    /// A pane is named by its callsign, which reaches it whatever it has been renamed, and a
    /// helper listed under its maker is indented a level for each generation.
    fn pane(
        &self,
        pane: &muster_proto::RosterPane,
        has_keyboard: bool,
        drawn: bool,
        depth: usize,
    ) -> String {
        let (widths, now_ms, say_machine) = (self.widths, self.now_ms, self.say_machine);
        let agent = self.states.get(pane.pane_id.as_str()).copied();
        let state = agent.map_or("unknown", |agent| agent.state.as_str());
        let held = agent.map(|agent| held_for(agent.since_ms, now_ms)).unwrap_or_default();
        let mut line = format!(
            "  {}{}  {}  {}  {}  {}",
            if has_keyboard { styled("▸", NAME) + " " } else { "  ".to_string() },
            pad(&place(pane.place), widths.place, QUIET, Align::Right),
            pad(&Helpers::called(pane, depth), widths.name, PLAIN, Align::Left),
            pad(state, widths.state, agent_style(state), Align::Left),
            pad(&held, widths.held, QUIET, Align::Right),
            beside_callsign(pane),
        );
        let waiting =
            agent.and_then(|agent| agent.facts.as_ref()).map(|facts| facts.waiting.as_str());
        // What it waits on says more than its title while it waits, and the two would crowd a line.
        // An unreachable pane shows neither: its title is the last one its daemon had, and would
        // read as what the pane is doing now.
        match waiting.filter(|waiting| state == "waiting" && !waiting.is_empty()) {
            Some(waiting) => line.push_str(&styled(&format!(" · on {waiting}"), QUIET)),
            None if !pane.subtitle.is_empty() && state != "unreachable" => {
                line.push_str(&styled(&format!(" · {}", pane.subtitle), QUIET));
            }
            None => {}
        }
        if let Some(agent) = agent {
            for said in agent_notes(agent) {
                line.push_str(&styled(&format!("  {said}"), QUIET));
            }
        }
        match self.helpers.count(pane) {
            0 => {}
            1 => line.push_str(&styled("  1 helper", QUIET)),
            count => line.push_str(&styled(&format!("  {count} helpers"), QUIET)),
        }
        match (depth > 0, pane.quiet) {
            (true, true) => line.push_str(&styled("  (hidden, quiet)", QUIET)),
            (true, false) => line.push_str(&styled("  (hidden)", QUIET)),
            (false, true) => line.push_str(&styled("  (quiet)", QUIET)),
            (false, false) => {}
        }
        // Only a window draws panes, so with none there is nothing to be off screen from.
        if drawn && !pane.on_screen {
            line.push_str(&styled("  (off screen)", QUIET));
        }
        // A helper not listed under its maker - in a tab of the window's, or with its maker gone
        // or out of sight - still says whose it is.
        if depth == 0 && !pane.maker.is_empty() {
            let maker = self.helpers.name_of(&pane.maker);
            line.push_str(&styled(&format!("  (helper of {maker})"), QUIET));
        }
        // Last on the row and only with more than one machine attached, because a tab may span two
        // and the pane is the only thing that can say which. On one machine it is on every row and
        // says nothing.
        if say_machine {
            line.push_str(&styled(&format!("  ({})", pane.daemon_id), QUIET));
        }
        line
    }
}

/// Which hidden helpers a window's listing prints under their maker's row rather than under
/// `hidden`, and what it says of every pane's helpers.
///
/// A hidden helper moves under its maker when the maker has a row of its own: a pane in a listed
/// tab, another window's, or a hidden pane, a helper among them. Everything else stays where it
/// is: a helper in one of the window's tabs keeps its place and the keyboard's mark there, and a
/// helper whose maker is not listed - closed, on a machine not attached, or drawn in `--layout`'s
/// boxes - has nowhere else to go.
#[derive(Default)]
struct Helpers<'a> {
    /// Each maker's hidden helpers that are listed under it, in the order the listing meets them.
    under: BTreeMap<&'a str, Vec<&'a muster_proto::RosterPane>>,
    /// Every helper in `under`, which its own tab no longer lists, with how far in its row is.
    moved: BTreeMap<&'a str, (&'a muster_proto::RosterPane, usize)>,
    /// How many helpers each maker has anywhere in the answer, moved or not.
    counts: BTreeMap<&'a str, usize>,
    /// What every pane in the answer is called, for a helper to name its maker by.
    names: BTreeMap<&'a str, &'a str>,
}

impl<'a> Helpers<'a> {
    fn of(window: &'a Window, others: Others, drawing: Option<&Drawing<'_>>) -> Helpers<'a> {
        let drawn = |tab: &muster_proto::RosterTab| {
            drawing.is_some_and(|drawing| drawing.layout(&tab.tab_id).is_some())
        };
        let other_tabs = window.windows.iter().filter(|other| others.include(other));
        let hidden: Vec<&muster_proto::RosterPane> =
            hidden_tabs(window).flat_map(|tab| tab.panes.iter()).collect();
        let listed: Vec<&muster_proto::RosterPane> = tabs(window)
            .chain(other_tabs.flat_map(|other| other.tabs.iter()))
            .filter(|tab| !drawn(tab))
            .flat_map(|tab| tab.panes.iter())
            .chain(hidden.iter().copied())
            .collect();
        let every = tabs(window)
            .chain(window.windows.iter().flat_map(|other| other.tabs.iter()))
            .flat_map(|tab| tab.panes.iter())
            .chain(hidden.iter().copied());

        let mut helpers = Helpers::default();
        for pane in every {
            helpers.names.insert(&pane.pane_id, callsign_or_id(pane));
            if !pane.maker.is_empty() {
                *helpers.counts.entry(&pane.maker).or_default() += 1;
            }
        }
        let ids: Vec<&str> = listed.iter().map(|pane| pane.pane_id.as_str()).collect();
        let makers: Vec<(&str, Option<&str>)> = hidden
            .iter()
            .map(|pane| {
                (pane.pane_id.as_str(), Some(pane.maker.as_str()).filter(|m| !m.is_empty()))
            })
            .collect();
        let by_id: BTreeMap<&str, &muster_proto::RosterPane> =
            hidden.iter().map(|pane| (pane.pane_id.as_str(), *pane)).collect();
        for nested in muster_core::helpers::nest(&ids, &makers) {
            let helper = by_id[nested.helper];
            helpers.moved.insert(&helper.pane_id, (helper, nested.depth));
            helpers.under.entry(&helper.maker).or_default().push(helper);
        }
        helpers
    }

    fn moved(&self, pane: &muster_proto::RosterPane) -> bool {
        self.moved.contains_key(pane.pane_id.as_str())
    }

    fn under(&self, maker: &muster_proto::RosterPane) -> &[&'a muster_proto::RosterPane] {
        self.under.get(maker.pane_id.as_str()).map_or(&[], Vec::as_slice)
    }

    fn count(&self, maker: &muster_proto::RosterPane) -> usize {
        self.counts.get(maker.pane_id.as_str()).copied().unwrap_or_default()
    }

    /// A maker by its callsign when the answer lists it, and by its id when it does not.
    fn name_of<'b>(&'b self, maker: &'b str) -> &'b str {
        self.names.get(maker).copied().unwrap_or(maker)
    }

    /// Each helper listed under its maker, with how many levels in its row is.
    fn rows(&self) -> impl Iterator<Item = (&'a muster_proto::RosterPane, usize)> + '_ {
        self.moved.values().copied()
    }

    /// What a row names a pane by: its callsign, indented a level for each generation of
    /// helper.
    fn called(pane: &muster_proto::RosterPane, depth: usize) -> String {
        format!("{}{}", "  ".repeat(depth), callsign_or_id(pane))
    }
}

/// The callsign a pane answers to, or its id from a daemon that draws none.
fn callsign_or_id(pane: &muster_proto::RosterPane) -> &str {
    if pane.callsign.is_empty() { &pane.pane_id } else { &pane.callsign }
}

/// What follows a pane's callsign: its label with the callsign taken out, so it is said once.
fn beside_callsign(pane: &muster_proto::RosterPane) -> &str {
    if pane.callsign.is_empty() { &pane.label } else { &pane.beside_callsign }
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
    // The swatch's color as text rather than painted: what the CLI paints is the terminal's
    // sixteen, and a hex triple is not one of them.
    let color =
        if machine.color.is_empty() { String::new() } else { format!(" · {}", machine.color) };
    lines.push(styled(&format!("  {where_it_runs} · {whose} · {holding}{color}"), QUIET));
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

/// What else is worth a glance about a pane's agent, each a few words: a program that has
/// stopped reading, how full its context is, its sub-agents, whether its screen can be read, an
/// adapter that could report and does not, a bell nobody heard, and a program's progress. For a
/// pane whose daemon cannot be reached, only what it read before.
///
/// The model and the cost are left to `--json`: they change what somebody does next less often
/// than they would lengthen every line.
fn agent_notes(agent: &muster_proto::PaneStateChanged) -> Vec<String> {
    // Everything else here is a claim about the present, which a window that cannot reach the
    // pane's daemon cannot make. What the pane read before is the one thing worth keeping.
    if agent.state == "unreachable" {
        return (!agent.last_state.is_empty())
            .then(|| format!("(was {})", agent.last_state))
            .into_iter()
            .collect();
    }
    let mut notes = Vec::new();
    // First, since it says why the state is what it is. On any pane, agent or not: a shell's
    // program that stopped reading is just as unable to take what is sent to it.
    if let Some(unread) = &agent.unread_input {
        notes.push(format!("({} has not read {} of input)", unread.program, bytes(unread.bytes)));
    }
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
    let queued = match agent.compaction.as_str() {
        "queued" => Some("compaction"),
        "follow_up" => Some("follow-up"),
        _ => None,
    };
    if let Some(queued) = queued {
        let subagents = agent.facts.as_ref().map_or(0, |facts| facts.subagents);
        let waits = AgentState::from_backend(&agent.state).line_waits_for(subagents);
        notes.push(format!("({queued} queued, waiting for {waits})"));
    }
    // Only where there is an adapter to install: a harness without one is silent by nature.
    if agent.adapter == "silent" {
        notes.push("(adapter not reporting)".to_string());
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

fn bytes(count: u32) -> String {
    if count == 1 { "1 byte".to_string() } else { format!("{count} bytes") }
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
/// Stuck is magenta, which no other state has and the window's pink is nearest to: an agent that
/// will not go on until somebody frees its program. Not red, which the window keeps for a nearly
/// full context ring so that a ring is never read as an agent asking for something.
///
/// Only the states worth a colour get one. `unknown` is the ordinary answer for a pane running a
/// shell rather than an agent, and colouring it would put a signal on almost every row; `idle` goes
/// bare for the reason the window draws it no border, that no colour is what resting looks like in
/// a list of words and the row already prints the word.
fn agent_style(state: &str) -> Style {
    match state {
        "working" => Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Cyan))),
        "blocked" => Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Yellow))),
        "stuck" => Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Magenta))),
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
        helpers: &Helpers<'_>,
    ) -> Widths {
        let mut widths = Widths { place: 1, name: 0, state: 0, held: 0 };
        let mut fit = |pane: &muster_proto::RosterPane, depth: usize| {
            widths.place = widths.place.max(pane.place.to_string().width());
            widths.name = widths.name.max(Helpers::called(pane, depth).width());
            let agent = states.get(pane.pane_id.as_str());
            let state = agent.map_or("unknown", |agent| agent.state.as_str());
            widths.state = widths.state.max(state.width());
            let held = agent.map(|agent| held_for(agent.since_ms, now_ms)).unwrap_or_default();
            widths.held = widths.held.max(held.width());
        };
        // A helper listed under its maker is a row among the window's own, so it is measured
        // with them.
        for (helper, depth) in helpers.rows() {
            fit(helper, depth);
        }
        for tab in tabs(window) {
            for pane in &tab.panes {
                fit(pane, 0);
            }
        }
        for tab in tabs(window) {
            widths.name = widths.name.max(tab.tab_id.width());
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

/// A tab this window neither places nor draws - another window's, or a hidden one - with its
/// panes and what each is doing.
fn unplaced_tab_json(
    tab: &muster_proto::RosterTab,
    states: &BTreeMap<&str, &muster_proto::PaneStateChanged>,
) -> Value {
    json!({
        "tab": tab.tab_id,
        "daemons": tab.daemon_ids,
        "label": tab.label,
        "given_name": tab.given_name,
        "panes": tab.panes.iter().map(|pane| with_agent(json!({
            "pane": pane.pane_id,
            "callsign": (!pane.callsign.is_empty()).then_some(&pane.callsign),
            "name": (!pane.name.is_empty()).then_some(&pane.name),
            "daemon": pane.daemon_id,
            "label": pane.label,
            "quiet": pane.quiet,
            "maker": maker_json(pane),
        }), states.get(pane.pane_id.as_str()).copied())).collect::<Vec<Value>>(),
    })
}

/// The id of the pane that made this one as its helper, or null for a pane nobody made as one.
fn maker_json(pane: &muster_proto::RosterPane) -> Value {
    if pane.maker.is_empty() { Value::Null } else { json!(pane.maker) }
}

/// A pane's row with what its agent is doing added, the same fields wherever the pane is listed:
/// a script that reads `unread_input` off a pane in this window reads it off a hidden one too.
///
/// An unreachable pane leaves out every field that would read as the present - its facts, its
/// progress, its unread input, a bell, whether it reports, and its title - so a filter such as
/// `.facts.context_used > 80` does not find an agent that may be gone. `last_state` says what it
/// was.
fn with_agent(mut row: Value, agent: Option<&muster_proto::PaneStateChanged>) -> Value {
    let said = json!({
        "state": agent.map_or("unknown", |agent| agent.state.as_str()),
        "last_state": agent.map(|agent| agent.last_state.as_str()).filter(|last| !last.is_empty()),
        "since": agent.map_or(Value::Null, |agent| since_json(agent.since_ms)),
        "reported": agent.is_some_and(|agent| agent.reported),
        "unreadable": agent.is_some_and(|agent| agent.unreadable),
        "facts": agent.and_then(|agent| agent.facts.as_ref()).map_or(Value::Null, facts_json),
        "progress": agent.and_then(|agent| agent.progress.as_ref()).map_or(Value::Null, |progress| {
            json!({ "state": progress.state, "percent": progress.percent })
        }),
        "rang": agent.is_some_and(|agent| agent.rang),
        "adapter": agent.map(|agent| agent.adapter.as_str()).filter(|adapter| !adapter.is_empty()),
        "compaction": agent.map(|agent| agent.compaction.as_str()).filter(|queued| !queued.is_empty()),
        "unread_input": agent.and_then(|agent| agent.unread_input.as_ref()).map_or(Value::Null, |unread| {
            json!({
                "bytes": unread.bytes,
                "since": since_json(unread.since_ms),
                "program": unread.program,
                "pid": unread.pid,
            })
        }),
        "session": agent.map(|agent| agent.session_id.as_str()).filter(|session| !session.is_empty()),
        "resume": agent.and_then(|agent| agent.resume.as_ref()).map_or(Value::Null, |resume| {
            json!({
                "command": resume.command,
                "uncarried": Some(&resume.uncarried).filter(|why| !why.is_empty()),
            })
        }),
    });
    if let (Value::Object(row_fields), Value::Object(said)) = (&mut row, said) {
        row_fields.extend(said);
        if agent.is_some_and(|agent| agent.state == "unreachable") {
            for stale in ["facts", "progress", "unread_input", "rang", "reported", "subtitle"] {
                row_fields.remove(stale);
            }
        }
    }
    row
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
            panes.push(with_agent(json!({
                "pane": pane.pane_id,
                // What to call the pane: `--pane` and `--to` take it, renamed or not. Null only
                // from a daemon that draws none.
                "callsign": (!pane.callsign.is_empty()).then_some(&pane.callsign),
                // The name somebody gave it, or else its callsign; taken by `--pane` and `--to`
                // too.
                "name": (!pane.name.is_empty()).then_some(&pane.name),
                "place": pane.place,
                "daemon": pane.daemon_id,
                "tab": tab.tab_id,
                "label": pane.label,
                "given_name": pane.given_name,
                "subtitle": pane.subtitle,
                "quiet": pane.quiet,
                // The pane that made this one as its helper, so `select(.maker == $MUSTER_PANE)`
                // finds an agent's helpers wherever each is held.
                "maker": maker_json(pane),
                "on_screen": pane.on_screen,
                "keyboard": keyboard.as_deref() == Some(pane.pane_id.as_str()),
                // Null rather than zeroes for a pane the window is not drawing, so this and
                // `on_screen` above cannot say different things: a rectangle of no size is a
                // place, and a pane behind a zoom or in a background tab has none.
                "rect": places.get(pane.pane_id.as_str()).copied().map_or(Value::Null, rect_json),
            }), states.get(pane.pane_id.as_str()).copied()));
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
                "color": (!daemon.color.is_empty()).then_some(&daemon.color),
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
                "tabs": other.tabs.iter().map(|tab| unplaced_tab_json(tab, &states)).collect::<Vec<Value>>(),
            })
        })
        .collect();
    add_layout(window, &mut tabs_out, &mut panes, &mut other_windows);
    // In the shape other windows' tabs take, and for the same reason: nothing here places or
    // draws them.
    let hidden: Vec<Value> =
        hidden_tabs(window).map(|tab| unplaced_tab_json(tab, &states)).collect();

    json!({
        "name": window.name,
        "other_windows": other_windows,
        "hidden": hidden,
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
    // Empty from a window older than this CLI, which had no version to give.
    let version =
        if daemon.version.is_empty() { String::new() } else { format!(" · {}", daemon.version) };
    match daemon.state.as_str() {
        "answering" if daemon.panes == 0 => format!("answering{version} · holding nothing"),
        "answering" => format!(
            "answering{version} · {} pane(s){}",
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
                "version": daemon.version,
            }))
            .collect::<Vec<Value>>(),
    })
}

/// `text` as one word a POSIX shell reads back unchanged, for a command printed to be pasted:
/// as it is when it needs no quoting, which is every path Muster makes, and single-quoted
/// otherwise.
pub(crate) fn shell_word(text: &str) -> String {
    let plain = !text.is_empty()
        && text.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@%,=".contains(c));
    if plain { text.to_string() } else { format!("'{}'", text.replace('\'', "'\\''")) }
}

#[cfg(test)]
mod tests {
    use super::{
        Others, QUIET, agent_style, daemons_text, held_for, window_json, window_text, windows,
    };
    use anstyle::{AnsiColor, Color, Style};

    /// A `pane new` nobody answered names the pane it may have made as a field, so a `--json`
    /// caller finds it without reading the sentence; any other unanswered request has none.
    #[test]
    fn an_unanswered_request_that_makes_a_pane_carries_its_name() {
        use muster_proto::{Response, Unanswered, response};
        let unanswered = |pane: &str| Response {
            payload: Some(response::Payload::Unanswered(Unanswered {
                reason: "the daemon did not answer".to_string(),
                pane_id: pane.to_string(),
            })),
        };
        match super::answer(&unanswered("p1w3r07bsd"), true) {
            Err(crate::Trouble::MayHaveMade { pane, .. }) => assert_eq!(pane, "p1w3r07bsd"),
            other => panic!("answered {other:?}"),
        }
        assert!(matches!(super::answer(&unanswered(""), true), Err(crate::Trouble::Unanswered(_))));
    }

    /// An app that would not say what it holds is still a row with every key the others have,
    /// so a filter over `.windows[]` reads it without a special case.
    #[test]
    fn an_app_that_would_not_answer_lists_every_key() {
        let answers = vec![(
            "/s/command-1.sock".to_string(),
            Err(crate::Trouble::Unreachable("nothing answered".to_string())),
        )];
        let listed: serde_json::Value =
            serde_json::from_str(&windows(&answers, None, false, true)).expect("JSON");
        let row = &listed["windows"][0];
        for key in ["window", "open", "here", "socket", "unreadable"] {
            assert!(row.get(key).is_some(), "the row has no {key}: {row}");
        }
    }

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

    /// An answering daemon says which build it is, since that decides whether a newer Muster
    /// takes its panes over; a window too old to say leaves the headline as it was.
    #[test]
    fn an_answering_daemon_says_its_version_when_the_window_knows_it() {
        let daemon = |socket: &str, version: &str| muster_proto::KnownDaemon {
            socket: socket.to_string(),
            state: "answering".to_string(),
            panes: 2,
            version: version.to_string(),
            ..muster_proto::KnownDaemon::default()
        };
        let text = daemons_text(&muster_proto::Daemons {
            remembered: true,
            daemons: vec![
                daemon("/home/a/.muster/daemon/release.sock", "0.17.1-dev"),
                daemon("/home/a/.muster/daemon/dev-0a1b2c3d4e5f.sock", ""),
            ],
        });
        assert!(text.contains("answering · 0.17.1-dev · 2 pane(s)"), "{text}");
        assert!(text.contains("answering · 2 pane(s)"), "{text}");
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
        assert_eq!(agent_style("stuck"), hue(AnsiColor::Magenta), "the window paints stuck pink");
    }

    /// An adapter that could report and does not is a note on the pane's row; one with nothing to
    /// say yet is no note, and null in `--json`.
    #[test]
    fn a_silent_adapter_is_noted_and_an_unsaid_one_is_not() {
        use muster_proto::{PaneStateChanged, RosterChanged, RosterPane, RosterTab, Window};
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
            panes: [("p1", "silent"), ("p2", "")]
                .map(|(pane, adapter)| PaneStateChanged {
                    pane_id: pane.to_string(),
                    state: "idle".to_string(),
                    adapter: adapter.to_string(),
                    ..PaneStateChanged::default()
                })
                .to_vec(),
            ..Window::default()
        };
        let json = window_json(&window, Others::All);
        assert_eq!(json["panes"][0]["adapter"], "silent", "{json}");
        assert!(json["panes"][1]["adapter"].is_null(), "{json}");
        let text = window_text(&window, 0, Others::All, false);
        let row = |pane: &str| text.lines().find(|line| line.contains(pane)).unwrap_or_default();
        assert!(row("p1").contains("(adapter not reporting)"), "{text}");
        assert!(!row("p2").contains("adapter"), "{text}");
    }

    /// A program that stopped reading is a note on its row, naming it and what it left, and an
    /// object in `--json`; a pane whose program reads has neither.
    #[test]
    fn a_program_that_stopped_reading_is_noted() {
        use muster_proto::{
            PaneStateChanged, RosterChanged, RosterPane, RosterTab, UnreadInput, Window,
        };
        let unread = UnreadInput {
            bytes: 2817,
            since_ms: 1_700_000_000_000,
            program: "codex".to_string(),
            pid: 4242,
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
            panes: vec![
                PaneStateChanged {
                    pane_id: "p1".to_string(),
                    state: "stuck".to_string(),
                    unread_input: Some(unread),
                    ..PaneStateChanged::default()
                },
                PaneStateChanged {
                    pane_id: "p2".to_string(),
                    state: "idle".to_string(),
                    ..PaneStateChanged::default()
                },
            ],
            ..Window::default()
        };
        let json = window_json(&window, Others::All);
        assert_eq!(json["panes"][0]["state"], "stuck", "{json}");
        assert_eq!(json["panes"][0]["unread_input"]["bytes"], 2817, "{json}");
        assert_eq!(json["panes"][0]["unread_input"]["program"], "codex", "{json}");
        assert!(json["panes"][1]["unread_input"].is_null(), "{json}");
        let text = window_text(&window, 0, Others::All, false);
        let row = |pane: &str| text.lines().find(|line| line.contains(pane)).unwrap_or_default();
        assert!(row("p1").contains("stuck"), "{text}");
        assert!(row("p1").contains("(codex has not read 2817 bytes of input)"), "{text}");
        assert!(!row("p2").contains("has not read"), "{text}");
    }

    /// A pane's agent session and how it resumes are in `--json`, so a script can restart or
    /// hand over a session without reading the harness's own files; null where there is none.
    #[test]
    fn window_json_gives_an_agents_session_and_its_resume() {
        use muster_proto::{
            PaneStateChanged, RosterChanged, RosterPane, RosterTab, SessionResume, Window,
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
            panes: vec![
                PaneStateChanged {
                    pane_id: "p1".to_string(),
                    state: "idle".to_string(),
                    session_id: "s-1".to_string(),
                    resume: Some(Box::new(SessionResume {
                        command: ["claude", "--model", "opus", "--resume", "s-1"]
                            .map(str::to_string)
                            .to_vec(),
                        uncarried: String::new(),
                    })),
                    ..PaneStateChanged::default()
                },
                PaneStateChanged {
                    pane_id: "p2".to_string(),
                    state: "idle".to_string(),
                    ..PaneStateChanged::default()
                },
            ],
            ..Window::default()
        };
        let json = window_json(&window, Others::All);
        assert_eq!(json["panes"][0]["session"], "s-1", "{json}");
        assert_eq!(
            json["panes"][0]["resume"],
            serde_json::json!({
                "command": ["claude", "--model", "opus", "--resume", "s-1"],
                "uncarried": null,
            }),
            "{json}"
        );
        assert!(json["panes"][1]["session"].is_null(), "{json}");
        assert!(json["panes"][1]["resume"].is_null(), "{json}");
    }

    /// A compaction queued for an agent says on its row what it waits for, naming the sub-agents
    /// that can hold a turn open, and `--json` says which line is queued.
    #[test]
    fn a_queued_compaction_says_what_it_waits_for() {
        use muster_proto::{
            AgentFacts, PaneStateChanged, RosterChanged, RosterPane, RosterTab, Window,
        };
        let window = Window {
            roster: Some(RosterChanged {
                tabs: vec![RosterTab {
                    tab_id: "t1".to_string(),
                    panes: ["p1", "p2", "p3"]
                        .map(|pane| RosterPane {
                            pane_id: pane.to_string(),
                            ..RosterPane::default()
                        })
                        .to_vec(),
                    ..RosterTab::default()
                }],
                ..RosterChanged::default()
            }),
            panes: vec![
                PaneStateChanged {
                    pane_id: "p1".to_string(),
                    state: "working".to_string(),
                    facts: Some(AgentFacts { subagents: 1, ..AgentFacts::default() }),
                    compaction: "queued".to_string(),
                    ..PaneStateChanged::default()
                },
                PaneStateChanged {
                    pane_id: "p2".to_string(),
                    state: "idle".to_string(),
                    compaction: "follow_up".to_string(),
                    ..PaneStateChanged::default()
                },
                PaneStateChanged {
                    pane_id: "p3".to_string(),
                    state: "working".to_string(),
                    ..PaneStateChanged::default()
                },
            ],
            ..Window::default()
        };
        let json = window_json(&window, Others::All);
        assert_eq!(json["panes"][0]["compaction"], "queued", "{json}");
        assert_eq!(json["panes"][1]["compaction"], "follow_up", "{json}");
        assert!(json["panes"][2]["compaction"].is_null(), "{json}");
        let text = window_text(&window, 0, Others::All, false);
        let row = |pane: &str| text.lines().find(|line| line.contains(pane)).unwrap_or_default();
        assert!(
            row("p1")
                .contains("(compaction queued, waiting for its 1 sub-agent and its turn to end)"),
            "{text}"
        );
        assert!(row("p2").contains("(follow-up queued, waiting for an empty prompt)"), "{text}");
        assert!(!row("p3").contains("queued"), "{text}");
    }

    /// A hidden pane's row in `--json` says what its agent is doing in the same fields a row in
    /// the window does, so one filter reads both.
    #[test]
    fn a_hidden_pane_carries_what_its_agent_is_doing() {
        use muster_proto::{
            AgentFacts, PaneStateChanged, RosterChanged, RosterPane, RosterTab, UnreadInput, Window,
        };
        let window = Window {
            roster: Some(RosterChanged {
                hidden: vec![RosterTab {
                    tab_id: "t9".to_string(),
                    panes: vec![RosterPane { pane_id: "p9".to_string(), ..RosterPane::default() }],
                    ..RosterTab::default()
                }],
                ..RosterChanged::default()
            }),
            panes: vec![PaneStateChanged {
                pane_id: "p9".to_string(),
                state: "stuck".to_string(),
                since_ms: 1_700_000_000_000,
                facts: Some(AgentFacts { context_used: Some(0.4), ..AgentFacts::default() }),
                unread_input: Some(UnreadInput {
                    bytes: 12,
                    program: "codex".to_string(),
                    ..UnreadInput::default()
                }),
                ..PaneStateChanged::default()
            }],
            ..Window::default()
        };
        let json = window_json(&window, Others::All);
        let hidden = &json["hidden"][0]["panes"][0];
        assert_eq!(hidden["state"], "stuck", "{json}");
        assert_eq!(hidden["since"], 1_700_000_000.0, "{json}");
        assert_eq!(hidden["unread_input"]["program"], "codex", "{json}");
        assert!(hidden["facts"]["context_used"].as_f64().is_some_and(|used| used > 0.39), "{json}");
    }

    /// Every pane in `--json` says whose helper it is, wherever it is held, and null for a pane
    /// nobody made as one.
    #[test]
    fn every_pane_says_whose_helper_it_is() {
        use muster_proto::{RosterChanged, RosterPane, RosterTab, Window};
        let pane = |id: &str, maker: &str| RosterPane {
            pane_id: id.to_string(),
            maker: maker.to_string(),
            ..RosterPane::default()
        };
        let tab =
            |id: &str, panes| RosterTab { tab_id: id.to_string(), panes, ..RosterTab::default() };
        let window = Window {
            roster: Some(RosterChanged {
                tabs: vec![tab("t1", vec![pane("p1", ""), pane("p2", "p1")])],
                hidden: vec![tab("t2", vec![pane("p3", "p1")])],
                ..RosterChanged::default()
            }),
            ..Window::default()
        };
        let json = window_json(&window, Others::All);
        assert!(json["panes"][0]["maker"].is_null(), "{json}");
        assert_eq!(json["panes"][1]["maker"], "p1", "{json}");
        assert_eq!(json["hidden"][0]["panes"][0]["maker"], "p1", "{json}");
    }

    /// A window with a maker on screen, its hidden helpers and a helper of one of them, a helper
    /// split beside it, and a hidden helper of a pane no window lists.
    fn helpers_window() -> muster_proto::Window {
        use muster_proto::{PaneStateChanged, RosterChanged, RosterPane, RosterTab, Window};
        let pane = |id: &str, word: &str, maker: &str, quiet: bool| RosterPane {
            pane_id: id.to_string(),
            callsign: word.to_string(),
            name: word.to_string(),
            label: format!("🤖 {word}"),
            beside_callsign: format!("🤖 {word}"),
            maker: maker.to_string(),
            quiet,
            ..RosterPane::default()
        };
        let tab =
            |id: &str, panes| RosterTab { tab_id: id.to_string(), panes, ..RosterTab::default() };
        Window {
            roster: Some(RosterChanged {
                tabs: vec![tab(
                    "t1",
                    vec![pane("p1", "lead", "", false), pane("p2", "beside", "p1", false)],
                )],
                hidden: vec![
                    tab(
                        "t2",
                        vec![
                            pane("p3", "otter", "p1", false),
                            pane("p4", "maple", "p1", true),
                            pane("p5", "cobalt", "p3", false),
                        ],
                    ),
                    tab("t3", vec![pane("p6", "stray", "p9gone", false)]),
                ],
                ..RosterChanged::default()
            }),
            panes: ["p1", "p2", "p3", "p4", "p5", "p6"]
                .map(|pane| PaneStateChanged {
                    pane_id: pane.to_string(),
                    state: "idle".to_string(),
                    ..PaneStateChanged::default()
                })
                .to_vec(),
            ..Window::default()
        }
    }

    fn unstyled(text: &str) -> String {
        anstream::adapter::strip_str(text).to_string()
    }

    /// A maker's hidden helpers are listed under its row, by their callsigns as every pane is, a
    /// level in for each generation, and their tab leaves the hidden list once it has nothing
    /// else in it.
    #[test]
    fn hidden_helpers_are_listed_under_their_maker() {
        let text = unstyled(&window_text(&helpers_window(), 0, Others::All, false));
        let rows: Vec<&str> = text.lines().map(str::trim_end).collect();
        let lead = rows.iter().position(|row| row.contains("lead ")).expect("the maker is listed");
        let expected = [
            "lead        idle    🤖 lead  3 helpers",
            "  otter     idle    🤖 otter  1 helper  (hidden)",
            "    cobalt  idle    🤖 cobalt  (hidden)",
            "  maple     idle    🤖 maple  (hidden, quiet)",
            "beside      idle    🤖 beside  (helper of lead)",
        ];
        let listed: Vec<&str> = rows[lead..lead + 5].iter().map(|row| row.trim_start()).collect();
        let expected: Vec<&str> = expected.iter().map(|row| row.trim_start()).collect();
        assert_eq!(listed, expected, "\n{text}");
        // Each generation a level further in than its maker's name.
        let name_at = |row: usize, name: &str| rows[row].find(name);
        let maker_at = name_at(lead, "lead");
        let levels = [(1, "otter", 2), (2, "cobalt", 4), (3, "maple", 2)];
        for (row, name, indent) in levels {
            assert_eq!(name_at(lead + row, name), maker_at.map(|at| at + indent), "\n{text}");
        }
        assert!(!text.contains("t2"), "the helpers' tab has nothing left to list:\n{text}");
        // A helper whose maker nothing lists stays hidden, and says whose it is.
        let stray = rows.iter().position(|row| row.contains("stray")).expect("the stray is listed");
        assert!(rows[..stray].contains(&"hidden"), "{text}");
        assert!(rows[stray].ends_with("(helper of p9gone)"), "{text}");
    }

    /// The columns stay in line through an indented helper: its state starts where its maker's
    /// does.
    #[test]
    fn a_helpers_row_keeps_the_columns_in_line() {
        let text = unstyled(&window_text(&helpers_window(), 0, Others::All, false));
        let column = |pane: &str| {
            let row = text.lines().find(|row| row.contains(pane)).unwrap_or_default();
            row.find("idle")
        };
        assert_eq!(column("lead "), column("cobalt"), "{text}");
        assert_eq!(column("lead "), column("otter"), "{text}");
    }

    /// A pane whose daemon the window cannot reach says what it read before, and nothing that
    /// would pass for the present: no context, no colour of a live state, and `--json` keeps
    /// the old state apart from `state` so a filter on `blocked` does not find it.
    #[test]
    fn an_unreachable_pane_says_what_it_was_and_nothing_more() {
        use muster_proto::{
            AgentFacts, PaneStateChanged, RosterChanged, RosterPane, RosterTab, Window,
        };
        let window = Window {
            roster: Some(RosterChanged {
                tabs: vec![RosterTab {
                    tab_id: "t1".to_string(),
                    panes: ["p1", "p2"]
                        .map(|pane| RosterPane {
                            pane_id: pane.to_string(),
                            subtitle: "✳ Compacting".to_string(),
                            ..RosterPane::default()
                        })
                        .to_vec(),
                    ..RosterTab::default()
                }],
                ..RosterChanged::default()
            }),
            panes: vec![
                PaneStateChanged {
                    pane_id: "p1".to_string(),
                    state: "unreachable".to_string(),
                    last_state: "blocked".to_string(),
                    facts: Some(AgentFacts { context_used: Some(41.0), ..AgentFacts::default() }),
                    rang: true,
                    reported: true,
                    ..PaneStateChanged::default()
                },
                PaneStateChanged {
                    pane_id: "p2".to_string(),
                    state: "idle".to_string(),
                    ..PaneStateChanged::default()
                },
            ],
            ..Window::default()
        };
        let json = window_json(&window, Others::All);
        assert_eq!(json["panes"][0]["state"], "unreachable", "{json}");
        assert_eq!(json["panes"][0]["last_state"], "blocked", "{json}");
        assert!(json["panes"][1]["last_state"].is_null(), "{json}");
        for stale in ["facts", "progress", "unread_input", "rang", "reported", "subtitle"] {
            assert!(json["panes"][0].get(stale).is_none(), "{stale} read as current: {json}");
            assert!(json["panes"][1].get(stale).is_some(), "a live pane lost {stale}: {json}");
        }
        let text = window_text(&window, 0, Others::All, false);
        let row = |pane: &str| text.lines().find(|line| line.contains(pane)).unwrap_or_default();
        assert!(row("p1").contains("unreachable"), "{text}");
        assert!(row("p1").contains("(was blocked)"), "{text}");
        assert!(!row("p1").contains("context"), "a dead pane's context read as current: {text}");
        assert!(!row("p1").contains("Compacting"), "a dead pane's title read as current: {text}");
        assert!(row("p2").contains("Compacting"), "a live pane lost its title: {text}");
        assert!(!row("p2").contains("was"), "{text}");
        assert_eq!(
            agent_style("unreachable"),
            QUIET,
            "an unreachable pane took a live state's colour"
        );
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

    /// A zoom in the part holding the keyboard fills the whole tab, so another machine's part is
    /// behind it too rather than drawn beside it.
    #[test]
    fn a_zoom_with_the_keyboard_is_drawn_filling_a_tab_on_two_machines() {
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
                    panes: ["p1", "p2", "p3"]
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
                focused_region: "r1".to_string(),
                regions: vec![
                    ViewRegion {
                        region_id: "r0".to_string(),
                        daemon_id: "laptop".to_string(),
                        pane_id: "p1".to_string(),
                        weight: 1.0,
                        root: Some(leaf("p1")),
                        ..ViewRegion::default()
                    },
                    ViewRegion {
                        region_id: "r1".to_string(),
                        daemon_id: "devenv".to_string(),
                        pane_id: "p3".to_string(),
                        zoomed: true,
                        weight: 1.0,
                        root: Some(ViewNode {
                            node: Some(view_node::Node::Split(Box::new(ViewSplit {
                                axis: "columns".to_string(),
                                ratio: 0.5,
                                first: Some(Box::new(leaf("p2"))),
                                second: Some(Box::new(leaf("p3"))),
                            }))),
                        }),
                        ..ViewRegion::default()
                    },
                ],
                ..TabLayout::default()
            }],
            grids: vec![grid("p1", 79), grid("p2", 39), grid("p3", 160)],
            ..Window::default()
        };
        let text = window_text(&window, 0, Others::All, false);
        assert!(text.contains("zoomed on p3"), "{text}");
        assert!(text.contains("160x40"), "the zoomed pane is drawn at its size: {text}");
        assert!(
            !text.contains("79x40"),
            "the other machine's pane is drawn beside the zoom: {text}"
        );
        assert!(text.contains("behind the zoom: p1, p2"), "{text}");
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
