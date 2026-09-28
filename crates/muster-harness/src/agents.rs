//! A pane's agent, driven the way a real one is: by what it paints.
//!
//! The daemon decides which agent a pane runs and what state it is in from the pane's
//! foreground process and its screen (MIP-3, section 8), and nothing can set either through the
//! protocol. So a test that needs a pane to be `working` or `blocked` runs the probe's fake
//! agent in it, under the name `claude`, with an override manifest that maps the markers the
//! fake paints onto states - and tells the fake which state to paint.

use std::path::Path;

use muster_daemon_proto::{self as proto, input_event};

use crate::daemon::Daemon;
use crate::input::Input;
use crate::requests::{snapshot, until_text};
use crate::until::until_some;

/// A shell script that paints `PROBE-STATE:<STATE>` and reads `working`, `blocked`, `idle`
/// or `quit`, one per line.
const FAKE_AGENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fake-agent/screen-agent");

/// Detection rules for `claude` that read the fake agent's markers. An override replaces the
/// built-in rules for its agent, which is safe because this daemon's home is the test's own.
const MANIFEST: &str = include_str!("../fake-agent/claude.toml");

/// What the fake agent is called in a daemon's home. Detection keys on the foreground
/// process's name, and the manifest is for `claude`.
const AGENT_NAME: &str = "claude";

/// Puts the manifest, and the fake agent under the name it is detected by, into a daemon's
/// home before it starts.
pub(crate) fn install(home: &Path) {
    let muster = home.join(".muster");
    let detection = muster.join("agent-detection");
    std::fs::create_dir_all(&detection).expect("the harness can make its detection directory");
    std::fs::write(detection.join("claude.toml"), MANIFEST).expect("the manifest is written");
    std::fs::create_dir_all(muster.join("bin")).expect("the harness can make its bin directory");
    std::os::unix::fs::symlink(FAKE_AGENT, muster.join("bin").join(AGENT_NAME))
        .expect("the fake agent is linked into the daemon's home");
}

impl Daemon {
    /// Starts the fake agent in `pane`, once its shell is at a prompt, and waits until it has
    /// painted and the daemon says it is idle.
    ///
    /// Waiting for the paint is not tidiness. The daemon names the agent as soon as its process
    /// is in the foreground, before the script has painted or started reading, and a command
    /// typed in that moment was not read for sixteen seconds.
    ///
    /// The daemon must be one [`Daemon::start_detecting`] started.
    pub fn run_agent(&self, pane: &str) {
        self.run_agent_with(pane, "");
    }

    /// [`Daemon::run_agent`], with the agent still starting as Claude Code starts: the first
    /// line typed into it fills its prompt and is not sent until a later Return.
    pub fn run_starting_agent(&self, pane: &str) {
        self.run_agent_with(pane, " starting");
    }

    fn run_agent_with(&self, pane: &str, arguments: &str) {
        let agent = self.root().join("home/.muster/bin").join(AGENT_NAME);
        assert!(agent.exists(), "run_agent needs a daemon from Daemon::start_detecting");
        let mut control = self.connect();
        until_text(&mut control, pane, "$");
        type_line(self, pane, &format!("{}{arguments}", agent.display()));
        until_text(&mut control, pane, "PROBE-STATE:IDLE");
        self.until_agent(pane, proto::AgentState::Idle);
    }

    /// Tells the fake agent in `pane` to be `working`, `blocked` or `idle`, and waits until
    /// the daemon says it is.
    pub fn set_agent_state(&self, pane: &str, state: proto::AgentState) {
        let command = match state {
            proto::AgentState::Working => "working",
            proto::AgentState::Blocked => "blocked",
            proto::AgentState::Idle => "idle",
            proto::AgentState::Unknown => panic!("the fake agent paints working, blocked or idle"),
        };
        type_line(self, pane, command);
        self.until_agent(pane, state);
    }

    /// Waits until the daemon says the fake agent in `pane` is in `state`.
    pub fn until_agent(&self, pane: &str, state: proto::AgentState) {
        let mut control = self.connect();
        until_some(&format!("{pane}'s agent to be {state:?}"), || {
            let record = snapshot(&mut control).panes.into_iter().find(|p| p.pane == pane)?;
            (record.agent.as_deref() == Some(AGENT_NAME) && record.agent_state() == state)
                .then_some(())
        });
    }
}

fn type_line(daemon: &Daemon, pane: &str, text: &str) {
    Input::connect(daemon.socket_path()).send(
        pane,
        input_event::Input::Send(input_event::Send { text: text.to_string(), enter: true }),
    );
}
