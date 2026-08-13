/// What the agent in a pane is doing.
///
/// Five, not four. `Unknown` is a state an agent can genuinely be in - a pane whose
/// harness we cannot classify - and it renders as itself. An agent we failed to read is
/// not an agent that finished.
///
/// `Done` is never something a backend stores. It is `Idle` on a pane nobody has seen
/// yet, derived wherever seen-ness is tracked, and Muster reads the derived value rather
/// than computing it. See `docs/architecture.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentState {
    Working,
    Blocked,
    Idle,
    Done,
    Unknown,
}

impl AgentState {
    /// Every state, so a test can assert the corpus covers them all.
    pub const ALL: [AgentState; 5] = [
        AgentState::Working,
        AgentState::Blocked,
        AgentState::Idle,
        AgentState::Done,
        AgentState::Unknown,
    ];

    /// Reads a backend's spelling of a state, treating anything unrecognized as `Unknown`.
    ///
    /// Backends are free to grow states we have never heard of - herdr's API is explicitly
    /// unstable and ships weekly. Failing closed onto `Unknown` means a Muster running
    /// against a newer daemon shows an honest "we don't know" instead of crashing or, far
    /// worse, quietly reading a novel state as `Idle` and telling the user nothing needs
    /// them.
    pub fn from_backend(value: &str) -> AgentState {
        match value {
            "working" => AgentState::Working,
            "blocked" => AgentState::Blocked,
            "idle" => AgentState::Idle,
            "done" => AgentState::Done,
            _ => AgentState::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::Blocked => "blocked",
            AgentState::Idle => "idle",
            AgentState::Done => "done",
            AgentState::Unknown => "unknown",
        }
    }
}
