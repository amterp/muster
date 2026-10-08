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
///
/// Nor is `Stuck`, though the daemon measures it: an agent whose program has stopped reading its
/// input, which the daemon puts on the pane's record beside whatever detection last read. The
/// screen shows what it showed before it stopped, so detection reads on as if nothing happened;
/// a stuck agent will not finish, answer a dialog or take a message until it reads again.
///
/// Nor is `Unreachable`, and no agent is ever in it. It is a pane whose daemon this window cannot
/// reach, so what the daemon last said is a guess about the present: a rebooted devenv's agents
/// are gone while its mirror still says `idle`. The window keeps that last state beside it rather
/// than under it, so nothing that filters on a state reads a dead pane as a live one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentState {
    Working,
    Blocked,
    Stuck,
    Waiting,
    Idle,
    Done,
    Unknown,
    Unreachable,
}

impl AgentState {
    /// Every state, so a test can assert the corpus covers them all.
    pub const ALL: [AgentState; 8] = [
        AgentState::Working,
        AgentState::Blocked,
        AgentState::Stuck,
        AgentState::Waiting,
        AgentState::Idle,
        AgentState::Done,
        AgentState::Unknown,
        AgentState::Unreachable,
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
            "stuck" => AgentState::Stuck,
            "waiting" => AgentState::Waiting,
            "idle" => AgentState::Idle,
            "done" => AgentState::Done,
            "unreachable" => AgentState::Unreachable,
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
    /// have finished, and one waiting on its own gate has not. So does `Stuck`: an agent that has
    /// stopped reading has not finished anything.
    pub fn counts_as(self, wanted: AgentState) -> bool {
        self == wanted || (self == AgentState::Done && wanted == AgentState::Idle)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::Blocked => "blocked",
            AgentState::Stuck => "stuck",
            AgentState::Waiting => "waiting",
            AgentState::Idle => "idle",
            AgentState::Done => "done",
            AgentState::Unknown => "unknown",
            AgentState::Unreachable => "unreachable",
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
            if word == AgentState::Unreachable.as_str() {
                return Err("`unreachable` cannot be waited for: a pane whose machine stops \
                            answering ends a wait on it as unanswered, and a wait on every pane \
                            carries on without it."
                    .to_string());
            }
            let Some(state) = AgentState::ALL.into_iter().find(|state| state.as_str() == word)
            else {
                let states: Vec<_> = AgentState::ALL
                    .iter()
                    .filter(|state| **state != AgentState::Unreachable)
                    .map(|state| state.as_str())
                    .collect();
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

    /// Whether a pane in `state` can no longer get where this waits for, so the wait ends refused
    /// rather than waiting on: a stuck agent gets nowhere until its program reads again, which
    /// nothing in a wait can make it do. A wait that asks for `stuck` is waiting for exactly that.
    pub fn stranded(&self, state: AgentState) -> bool {
        self.is_wait() && state == AgentState::Stuck && !self.states.contains(&AgentState::Stuck)
    }

    /// Why a wait on `pane` ends refused, once [`Until::stranded`] says it does: which program
    /// stopped reading, and how many bytes it left, when its daemon said.
    pub fn stranded_because(&self, pane: &str, program: Option<(&str, u32)>) -> String {
        let stopped = match program {
            Some((program, bytes)) => format!(
                "its program, {program}, has stopped reading its input and left {bytes} bytes \
                 unread"
            ),
            None => "its program has stopped reading its input".to_string(),
        };
        format!(
            "pane {pane} is stuck: {stopped}, so it will not be {} until it reads again, and \
             nothing a wait does can make it. Interrupting the program, or ending it and resuming \
             its session, frees it; `--until stuck` waits for a pane to get stuck instead.",
            self.spelled()
        )
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
