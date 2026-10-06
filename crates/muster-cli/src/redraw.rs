//! `muster window --watch --layout`: the layout drawn again each time the arrangement changes.
//!
//! The window cannot say when a drawing changed, only when it might have: after every publish
//! and every resize, most of which move nothing. So the watch is a doorbell, the layout is read
//! again on each ring, and a drawing is printed only when the arrangement in it differs from
//! the last one printed. A pane's state changing redraws nothing on its own.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use muster_proto::{Request, Response, response};
use serde_json::{Value, json};

use crate::{Trouble, dial, render, report, windowless};

/// How long a ring waits for the rest of its burst: one split publishes several times over as
/// each daemon answers, and one read covers them all.
const BURST: Duration = Duration::from_millis(150);

/// When to read once more after a drawing changed. A pane's size follows its arrangement by a
/// moment, since the terminal is resized after the divider moves, and nothing announces it.
const SETTLE: Duration = Duration::from_millis(500);

/// Clears a terminal and puts the cursor at its top left, so a drawing replaces the last one.
const CLEAR: &str = "\x1b[H\x1b[2J";

#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    watch: &Request,
    read: &Request,
    named: Option<&str>,
    no_window: bool,
    environment: &BTreeMap<String, String>,
    json: bool,
    out: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    let followed = match crate::follow(watch, named, no_window, environment) {
        Ok(followed) => followed,
        Err(trouble) => return report(&trouble, json, errors),
    };
    // The layout is read from wherever the watch is answered from, so a window passed over for
    // not answering is not waited on again at every drawing.
    let no_window = no_window || followed.by_the_daemon;
    let mut answers = crate::say_passed_over(followed, json, errors);
    // Read on a thread of its own, so this one can wait for a burst to end without a deadline on
    // the connection: a read that times out part way through a frame would lose the rest of it.
    let (doorbell, rings) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let answer = answers.next(None);
            let over = answer.is_err();
            if doorbell.send(answer).is_err() || over {
                return;
            }
        }
    });

    // A live view on a terminal, and a record of every drawing anywhere else.
    let in_place = !json && std::io::stdout().is_terminal();
    let mut last: Option<Value> = None;
    loop {
        let (arrangement, drawing) = match drawn(read, named, no_window, environment, json) {
            Ok(drawn) => drawn,
            Err(trouble) => return report(&trouble, json, errors),
        };
        let settle = if last.as_ref() == Some(&arrangement) {
            None
        } else {
            let lead = match (&last, in_place) {
                (_, true) => CLEAR,
                (Some(_), false) if !json => "\n",
                _ => "",
            };
            // The reader went away, as for any watch: nobody is left to tell.
            if writeln!(out, "{lead}{drawing}").and_then(|()| out.flush()).is_err() {
                return 0;
            }
            last = Some(arrangement);
            Some(Instant::now() + SETTLE)
        };

        // A pane's state changing is news to every other watch and moves nothing here, and
        // reading the layout again asks every daemon for every pane's size.
        let first = loop {
            let heard = match settle {
                Some(at) => rings.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => rings.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            if !matches!(&heard, Ok(Ok(response)) if is_a_state(response)) {
                break heard;
            }
        };
        let mut heard = match first {
            Ok(heard) => heard,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return 0,
        };
        loop {
            match heard {
                Ok(response) => {
                    if let Some(response::Payload::Failure(failure)) = &response.payload {
                        return report(&Trouble::Refused(failure.reason.clone()), json, errors);
                    }
                }
                Err(dial::Ended::HungUp(detail)) => {
                    return report(&Trouble::Unreachable(detail), json, errors);
                }
                // Asked with no deadline, so this cannot come; ending is the safe reading.
                Err(dial::Ended::TimedOut) => return 0,
            }
            heard = match rings.recv_timeout(BURST) {
                Ok(heard) => heard,
                Err(_) => break,
            };
        }
    }
}

/// The layout as it stands: what in it counts as the arrangement, and the drawing to print.
fn drawn(
    read: &Request,
    named: Option<&str>,
    no_window: bool,
    environment: &BTreeMap<String, String>,
    json: bool,
) -> Result<(Value, String), Trouble> {
    let (as_json, as_text) = match answered(read, named, no_window, environment)? {
        Answer::Window(response) => (
            render::answer(&response, true)?,
            (!json).then(|| render::answer(&response, false)).transpose()?,
        ),
        Answer::Daemon { window, socket } => (
            render::daemon_window(&window, &socket, true),
            (!json).then(|| render::daemon_window(&window, &socket, false)),
        ),
    };
    let value: Value = serde_json::from_str(&as_json).map_err(|error| {
        Trouble::Refused(format!(
            "muster could not read back its own answer ({error}); this is a bug in muster."
        ))
    })?;
    let drawing = as_text.unwrap_or_else(|| value.to_string());
    Ok((arrangement(&value), drawing.trim_end().to_string()))
}

enum Answer {
    Window(Response),
    Daemon { window: muster_proto::Window, socket: String },
}

/// The window's answer, or this machine's daemon's when no window is there to give one, chosen
/// as a one-off `muster window --layout` chooses.
fn answered(
    read: &Request,
    named: Option<&str>,
    no_window: bool,
    environment: &BTreeMap<String, String>,
) -> Result<Answer, Trouble> {
    let from_the_daemon = || match windowless::ask(read, environment)? {
        windowless::Answered::Window { window, socket } => {
            Ok(Answer::Daemon { window: *window, socket })
        }
        windowless::Answered::Response(response) => Ok(Answer::Window(*response)),
    };
    if no_window {
        return from_the_daemon();
    }
    match dial::ask(read, named, environment) {
        Err(trouble) if crate::instead_of(&trouble, read) => {
            from_the_daemon().map_err(|daemon| crate::neither(trouble.detail(), daemon))
        }
        asked => asked.map(Answer::Window),
    }
}

fn is_a_state(response: &Response) -> bool {
    matches!(response.payload, Some(response::Payload::PaneState(_)))
}

/// What a drawing changing means: where every tab and pane sits and how big each pane is, and
/// none of what the agents in them are doing.
fn arrangement(answer: &Value) -> Value {
    fn pick(value: &Value, keys: &[&str]) -> Value {
        Value::Object(
            keys.iter()
                .filter_map(|key| value.get(*key).map(|found| ((*key).to_string(), found.clone())))
                .collect(),
        )
    }
    fn each(list: &Value, of: impl Fn(&Value) -> Value) -> Value {
        Value::Array(list.as_array().into_iter().flatten().map(of).collect())
    }
    let region = |region: &Value| {
        let mut kept = pick(region, &["daemon", "weight", "zoomed", "layout"]);
        // Which pane a region names is where its keyboard is, except in a zoom, where it is the
        // pane filling the tab.
        if region["zoomed"] == json!(true) {
            kept["pane"] = region["pane"].clone();
        }
        kept
    };
    let pane = |pane: &Value| pick(pane, &["pane", "tab", "daemon", "frame", "cells"]);
    json!({
        "showing": answer["showing"],
        "tabs": each(&answer["tabs"], |tab| json!({
            "tab": tab["tab"],
            "regions": each(&tab["regions"], region),
        })),
        "panes": each(&answer["panes"], pane),
        "other_windows": each(&answer["other_windows"], |window| json!({
            "window": window["window"],
            "tabs": each(&window["tabs"], |tab| json!({
                "tab": tab["tab"],
                "panes": each(&tab["panes"], pane),
            })),
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state, a label or the keyboard moving is not the arrangement changing; a divider, a
    /// size or a tab moving is.
    #[test]
    fn only_the_arrangement_counts() {
        let answer = json!({
            "showing": "t1",
            "keyboard": "p1",
            "tabs": [{ "tab": "t1", "label": "x", "regions": [{
                "daemon": "local", "pane": "p1", "keyboard": true, "weight": 1.0,
                "zoomed": false, "layout": { "pane": "p1" },
            }]}],
            "panes": [{ "pane": "p1", "tab": "t1", "state": "idle", "label": "x",
                "frame": { "x": 0.0, "y": 0.0, "width": 1.0, "height": 1.0 },
                "cells": { "cols": 80, "rows": 24 } }],
        });
        let mut busy = answer.clone();
        busy["panes"][0]["state"] = json!("working");
        busy["panes"][0]["label"] = json!("renamed");
        busy["keyboard"] = Value::Null;
        busy["tabs"][0]["regions"][0]["keyboard"] = json!(false);
        assert_eq!(arrangement(&answer), arrangement(&busy));

        let mut resized = answer.clone();
        resized["panes"][0]["cells"]["cols"] = json!(100);
        assert_ne!(arrangement(&answer), arrangement(&resized));

        let mut moved = answer;
        moved["showing"] = json!("t2");
        assert_ne!(arrangement(&busy), arrangement(&moved));
    }
}
