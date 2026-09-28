//! Agent detection: which agent a pane is running, and what state it is in.
//!
//! A port of herdr v0.8.0's detection (Apache-2.0; see `NOTICE`), as a library with no PTY
//! and no socket. Screen text, the terminal's title and OSC 9 progress, and what the kernel
//! says about the pane's foreground processes go in; an agent and one of four states come
//! out. `done` is not among them: Muster derives it from whether anybody has looked
//! (`docs/architecture.md`, agent states).
//!
//! The agents are data. Each one is a manifest - the names its executables answer to and the
//! screen rules that classify it - built in, sent by the app, or overridden in
//! `~/.muster/agent-detection/`.

mod detector;
mod identify;
mod manifest;
mod manifests;
mod osc;
mod process;

use std::fmt;
use std::sync::Arc;

pub use detector::{Carried, CarriedReport, Detector, Drift, Pane, Publication, Tick};
pub use identify::{Probe, identify_in_job, probe};
pub use manifest::{Detection, ENGINE_VERSION, Input, Manifest, Prompt, Version};
pub use manifests::{Manifests, Source, Warning};
pub use osc::{Progress, title};
pub use process::{Job, Process, Processes, System};

/// What an agent is doing, as far as its screen says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Working,
    Blocked,
    Idle,
    Unknown,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Blocked => "blocked",
            State::Idle => "idle",
            State::Unknown => "unknown",
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An agent, by its manifest's id. An id rather than an index, so that an agent keeps its
/// identity across a reload of the manifests.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Agent(Arc<str>);

impl Agent {
    pub fn new(id: &str) -> Self {
        Agent(Arc::from(id))
    }

    pub fn id(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The detection text for a pane: its active screen as `muster_vt::Terminal::text(0, rows - 1)`
/// reads it - each row without trailing whitespace, trailing blank rows dropped, joined by
/// `\n` - and ended with a newline, as herdr's was. The newline is part of what the manifests
/// mean: a regex anchored with `$` against the whole region matches before it or not at all.
pub fn screen_text(rows: &str) -> String {
    let mut text = String::with_capacity(rows.len() + 1);
    text.push_str(rows);
    if !text.is_empty() {
        text.push('\n');
    }
    text
}
