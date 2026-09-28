//! Renders a capture from `tools/detection-capture.py` into corpus cases: each marked moment's
//! bytes through the terminal the daemon runs, then read the way the daemon reads a pane. The
//! expected state is the one the capture script knew the agent to be in, never this engine's
//! verdict - which is printed beside it, to stderr, for whoever is reviewing.

use std::path::PathBuf;

use muster_detect::{Agent, Input, Manifests, Progress, screen_text, title};
use muster_vt::Terminal;
use serde_json::{Value, json};

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: render_capture <capture dir>"));
    let raw = std::fs::read(dir.join("raw")).expect("the capture has no raw file");
    let marks: Value = serde_json::from_slice(
        &std::fs::read(dir.join("marks.json")).expect("the capture has no marks.json"),
    )
    .expect("marks.json is not JSON");
    let size = |key: &str| u16::try_from(marks[key].as_u64().expect("a size")).expect("a size");
    let (columns, rows) = (size("columns"), size("rows"));
    let manifests = Manifests::built_in();
    let claude = Agent::new("claude");

    let mut cases = Vec::new();
    for mark in marks["marks"].as_array().expect("marks") {
        let offset =
            usize::try_from(mark["offset"].as_u64().expect("an offset")).expect("an offset");
        let bytes = &raw[..offset];
        let mut terminal = Terminal::new(columns, rows).expect("a terminal");
        terminal.write(bytes);
        let mut progress = Progress::default();
        progress.observe(bytes);

        let screen = screen_text(&terminal.text(0, rows - 1));
        let title = title(&terminal.title());
        let progress = progress.get().to_string();
        let detection = manifests
            .detect(Some(&claude), Input { screen: &screen, title: &title, progress: &progress });
        eprintln!(
            "{}: expected {}, the engine says {} by {:?}",
            mark["name"], mark["expect"], detection.state, detection.rule
        );
        cases.push(json!({
            "name": mark["name"],
            "why": mark["note"],
            "given": { "agent": "claude", "screen": screen, "title": title, "progress": progress },
            "expect": if mark["skip"] == true {
                json!({ "state": mark["expect"], "skipStateUpdate": true })
            } else {
                json!({ "state": mark["expect"] })
            },
        }));
    }
    println!("{}", serde_json::to_string_pretty(&cases).expect("cases serialize"));
}
