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

/// What ends a wait on panes (`WatchPanes.until` and `context_at_least`): a state a pane gets
/// to, or its agent saying its context is at least so full, whichever comes first.
///
/// A condition rather than an event, so a pane already there meets it. Parsed in one place
/// because two watches evaluate it - the window's, and the CLI's own when no window answers -
/// and a wait that ended differently depending on which one answered would be two features.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Until {
    states: Vec<AgentState>,
    context_at_least: Option<f32>,
}

impl Until {
    /// Reads the state words a caller sent and the context it asked for, or says why there is
    /// nothing to wait for.
    pub fn parse(words: &[String], context_at_least: Option<f32>) -> Result<Until, String> {
        let mut states = Vec::new();
        for word in words {
            // Strict rather than `from_backend`, which reads a word it does not know as
            // `unknown`: a caller who typed `idel` would otherwise be waiting for a shell.
            let Some(state) = AgentState::ALL.into_iter().find(|state| state.as_str() == word)
            else {
                let states: Vec<_> = AgentState::ALL.iter().map(|state| state.as_str()).collect();
                return Err(format!(
                    "`{word}` is not a state a pane can be in, so there is nothing to wait for. \
                     The states are {}.",
                    states.join(", ")
                ));
            };
            states.push(state);
        }
        if let Some(percent) = context_at_least
            && !(percent > 0.0 && percent <= 100.0)
        {
            return Err(format!(
                "a wait on context {percent}% full can never end or has already ended for every \
                 agent; context is said in percent, so ask for more than 0 and at most 100."
            ));
        }
        Ok(Until { states, context_at_least })
    }

    /// Whether anything ends it. A watch with nothing to wait for runs until its caller leaves.
    pub fn is_wait(&self) -> bool {
        !self.states.is_empty() || self.context_at_least.is_some()
    }

    /// The context, in percent, that ends it on its own, if any.
    pub fn context_at_least(&self) -> Option<f32> {
        self.context_at_least
    }

    /// Whether a pane in `state`, whose agent last said its context was `context_used` percent
    /// full, has got where this is waiting for. A pane whose agent never said is not there:
    /// a harness without a report of its context cannot meet a wait on one.
    pub fn met(&self, state: AgentState, context_used: Option<f32>) -> bool {
        self.states.iter().any(|wanted| state.counts_as(*wanted))
            || self.context_at_least.zip(context_used).is_some_and(|(wanted, used)| used >= wanted)
    }

    /// What it waits for, as a sentence would put it: `idle or blocked`, `80% context`.
    pub fn spelled(&self) -> String {
        let mut said: Vec<String> =
            self.states.iter().map(|state| state.as_str().to_string()).collect();
        if let Some(percent) = self.context_at_least {
            said.push(format!("at {percent}% context"));
        }
        said.join(" or ")
    }
}
