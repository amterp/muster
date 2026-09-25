//! Which window holds each tab. Cases and their reasoning live in
//! corpus/conformance/tab-holding.json.

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::composition::{DaemonId, HeldWindow, Holders, Taker, WindowName};
use muster_core::mirror::backend::TabId;
use serde_json::{Value, json};

#[test]
fn tab_holding_conformance() {
    let corpus = Conformance::load("tab-holding.json");

    let ran = corpus.run(|given| {
        let mut holders = Holders::new();
        for step in given.get("steps").and_then(Value::as_array).into_iter().flatten() {
            act(&mut holders, step)?;
        }

        // Every case goes through the file as well. What a window decides is only ever what it
        // read back from the record, so a field the record drops is a decision that did not
        // survive the next window reading it.
        let written = to_toml(&holders);
        let read = from_toml(&written)
            .map_err(|error| CaseError::new(format!("the record did not read back: {error}")))?;
        if read != holders {
            return Err(CaseError::new(format!(
                "the record says something different after a round trip:\n{written}"
            )));
        }

        Ok(fields([
            ("windows", Some(json!(holders.windows().map(describe).collect::<Vec<_>>()))),
            ("tabs", Some(json!(describe_tabs(&holders)))),
            ("taker", given.get("taker").map(|asked| json!(taker(&holders, asked)))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn an_empty_record_is_one_nobody_has_written_yet() {
    // What the first window to open after this existed finds. Refusing it would leave that
    // window holding nothing and asking for a workspace beside every tab it used to show.
    assert_eq!(from_toml(""), Ok(Holders::new()));
}

#[test]
fn a_record_this_build_does_not_understand_is_refused_rather_than_guessed_at() {
    assert!(from_toml("version = 99\n").is_err());
    assert!(from_toml("this is not toml {{").is_err());
}

fn act(holders: &mut Holders, step: &Value) -> Result<(), CaseError> {
    match text(step, "do").as_str() {
        "open" => holders.opened(HeldWindow {
            name: window(step),
            arrangement: text(step, "arrangement"),
            socket: text(step, "socket"),
            pid: step
                .get("pid")
                .and_then(Value::as_u64)
                .and_then(|p| u32::try_from(p).ok())
                .unwrap_or(0),
            focused: number(step, "focused"),
        }),
        "close" => holders.closed(&window(step)),
        "focus" => holders.focused(&window(step), number(step, "at")),
        "take" => holders.take(TabId::new(text(step, "tab")), &window(step)),
        "expect" => {
            holders.expect(&window(step), &DaemonId::new(text(step, "daemon")), number(step, "at"));
        }
        "answered" => holders.expected(&window(step), &DaemonId::new(text(step, "daemon"))),
        "forget" => {
            let gone = strings(step, "windows");
            holders.forget(|window| gone.iter().any(|name| name == window.name.as_str()));
        }
        "prune" => {
            let known = strings(step, "known");
            holders.prune(|tab| known.iter().any(|name| name == tab.as_str()));
        }
        other => {
            return Err(CaseError::new(format!(
                "the case names a step the driver does not know: {other:?}"
            )));
        }
    }
    Ok(())
}

/// Asks who takes a tab nobody holds, with the windows the case says are open.
///
/// Which windows are open is the caller's to say, because the seam finds out by dialing each
/// one's socket - so a case names them rather than the record deciding from a pid.
fn taker(holders: &Holders, asked: &Value) -> String {
    let open = strings(asked, "open");
    let answer =
        holders.taker(&DaemonId::new(text(asked, "daemon")), number(asked, "now"), |window| {
            open.iter().any(|name| name == window.name.as_str())
        });
    match answer {
        Taker::Window(window) => window.to_string(),
        Taker::Waiting(window) => format!("waiting on {window}"),
        Taker::Nobody => "nobody".to_string(),
    }
}

fn describe(window: &HeldWindow) -> String {
    format!(
        "{} pid={} socket={} arrangement={} focused={}",
        window.name, window.pid, window.socket, window.arrangement, window.focused
    )
}

fn describe_tabs(holders: &Holders) -> Vec<String> {
    let mut tabs: Vec<String> = holders
        .windows()
        .flat_map(|window| {
            holders.held_by(&window.name).map(move |tab| format!("{tab} {}", window.name))
        })
        .collect();
    tabs.sort();
    tabs
}

fn window(step: &Value) -> WindowName {
    WindowName::new(text(step, "window"))
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn number(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}
