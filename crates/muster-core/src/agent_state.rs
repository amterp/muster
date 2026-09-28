/// What the agent in a pane is doing.
///
/// Five, not four. `Unknown` is a state an agent can genuinely be in - a pane whose
/// harness we cannot classify - and it renders as itself. An agent we failed to read is
/// not an agent that finished.
///
/// `Done` is never a daemon's agent state. It is an agent that finished while nobody looked:
/// the daemon holds the finish on the pane's record, and a window paints it `done` until
/// somebody sees it (`crate::attention`, `docs/architecture.md`).
///
/// Nor is `Waiting`. It is an idle agent that said it ended its turn to wait on work it started
/// itself, a gate or a build: it has not finished, and nobody is holding it up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentState {
    Working,
    Blocked,
    Waiting,
    Idle,
    Done,
    Unknown,
}

impl AgentState {
    /// Every state, so a test can assert the corpus covers them all.
    pub const ALL: [AgentState; 6] = [
        AgentState::Working,
        AgentState::Blocked,
        AgentState::Waiting,
        AgentState::Idle,
        AgentState::Done,
        AgentState::Unknown,
    ];

    /// Reads a backend's spelling of a state, treating anything unrecognized as `Unknown`.
    ///
    /// Backends are free to grow states we have never heard of - herdr's API, which Muster ran
    /// on first, was explicitly unstable and shipped weekly. Failing closed onto `Unknown`
    /// means a Muster running against a newer daemon shows an honest "we don't know" instead
    /// of crashing or, far worse, quietly reading a novel state as `Idle` and telling the user
    /// nothing needs them.
    pub fn from_backend(value: &str) -> AgentState {
        match value {
            "working" => AgentState::Working,
            "blocked" => AgentState::Blocked,
            "waiting" => AgentState::Waiting,
            "idle" => AgentState::Idle,
            "done" => AgentState::Done,
            _ => AgentState::Unknown,
        }
    }

    /// Whether a pane in this state has got where a caller waiting for `wanted` is waiting.
    ///
    /// The same state, or `Done` for a caller waiting on `Idle`. `done` is an idle nobody has
    /// looked at, and which of the two a pane reads is a fact about where somebody's cursor was -
    /// so a caller waiting for an agent to finish would otherwise wait forever on a window that
    /// happened not to be looked at, or return early on one that was. The reverse does not hold:
    /// a caller asking for `done` is asking about the unseen ones specifically.
    ///
    /// `Waiting` counts only as itself. A caller waiting for `idle` is waiting for an agent to
    /// have finished, and one waiting on its own gate has not.
    pub fn counts_as(self, wanted: AgentState) -> bool {
        self == wanted || (self == AgentState::Done && wanted == AgentState::Idle)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::Blocked => "blocked",
            AgentState::Waiting => "waiting",
            AgentState::Idle => "idle",
            AgentState::Done => "done",
            AgentState::Unknown => "unknown",
        }
    }
}
