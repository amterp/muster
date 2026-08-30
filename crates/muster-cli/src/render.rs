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
    }
}

/// A window as somebody reads it: each daemon, its tabs, and the panes in them.
fn window_text(window: &Window) -> String {
    let keyboard = keyboard_pane(window);
    let states = states(window);
    let widths = Widths::across(window, &states);

    let mut health: BTreeMap<&str, (&str, &str)> = BTreeMap::new();
    for daemon in &window.daemons {
        health.insert(&daemon.daemon_id, (&daemon.state, &daemon.detail));
    }

    let mut lines: Vec<String> = Vec::new();
    let roster = window.roster.iter().flat_map(|roster| roster.daemons.iter());
    for daemon in roster {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        let (state, detail) = health.remove(daemon.daemon_id.as_str()).unwrap_or(("unknown", ""));
        lines.push(daemon_line(&daemon.daemon_id, state, detail));

        if daemon.tabs.is_empty() {
            lines.push(styled("  no tabs, so this daemon is holding nothing", QUIET));
        }
        for tab in &daemon.tabs {
            lines.push(tab_line(&widths, tab));
            for pane in &tab.panes {
                lines.push(pane_line(
                    &widths,
                    pane,
                    states.get(pane.pane_id.as_str()).copied().unwrap_or("unknown"),
                    keyboard.as_deref() == Some(pane.pane_id.as_str()),
                ));
            }
        }
    }

    // A daemon Muster is following that the roster does not mention. Reported rather than dropped:
    // a window with a daemon it cannot describe is worth seeing, and a caller told nothing would
    // read it as a window with one fewer machine in it.
    for (daemon, (state, detail)) in health {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(daemon_line(daemon, state, detail));
        lines.push(styled("  nothing described yet", QUIET));
    }

    if lines.is_empty() {
        return "no daemon is attached, so this window is showing nothing".to_string();
    }
    lines.join("\n")
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
        "  {} {}  {}",
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
/// Only the states worth a colour get one. `unknown` is the ordinary answer for a pane running a
/// shell rather than an agent, and colouring it would put a signal on almost every row; `idle` goes
/// bare for the reason the window draws it no border, that no colour is what resting looks like in
/// a list of words and the row already prints the word.
fn agent_style(state: &str) -> Style {
    match state {
        "working" => Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Blue))),
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
        let tabs = window
            .roster
            .iter()
            .flat_map(|roster| roster.daemons.iter())
            .flat_map(|daemon| daemon.tabs.iter());
        for tab in tabs {
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

    let mut tabs = Vec::new();
    let mut panes = Vec::new();
    let roster = window.roster.iter().flat_map(|roster| roster.daemons.iter());
    for daemon in roster {
        for tab in &daemon.tabs {
            tabs.push(json!({
                "tab": tab.tab_id,
                "daemon": daemon.daemon_id,
                "place": tab.place,
                "label": tab.label,
                "given_name": tab.given_name,
                "on_screen": tab.on_screen,
            }));
            for pane in &tab.panes {
                panes.push(json!({
                    "pane": pane.pane_id,
                    "place": pane.place,
                    "daemon": daemon.daemon_id,
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
    }

    let daemons: Vec<Value> = window
        .daemons
        .iter()
        .map(|daemon| {
            json!({
                "daemon": daemon.daemon_id,
                "state": daemon.state,
                "detail": daemon.detail,
            })
        })
        .collect();

    // A pane says which tab holds it by name rather than by place, so one read is enough to act
    // on it: `.panes[] | select(.pane == $MUSTER_PANE) | .tab` is how a pane finds its own tab,
    // and there is nothing in a pane's environment that says. The place is still in `tabs[]` for
    // anyone who wants it.
    json!({ "daemons": daemons, "keyboard": keyboard, "tabs": tabs, "panes": panes })
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
        assert_eq!(agent_style("working"), hue(AnsiColor::Blue), "the window paints working blue");
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
