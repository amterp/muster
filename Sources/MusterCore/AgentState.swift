/// What the agent in a pane is doing.
///
/// Five, not four. `unknown` is a state an agent can genuinely be in - a pane whose
/// harness we cannot classify - and it renders as itself. An agent we failed to read is
/// not an agent that finished.
///
/// `done` is never something a backend stores. It is `idle` on a pane nobody has seen
/// yet, derived wherever seen-ness is tracked, and Muster reads the derived value
/// rather than computing it. See `docs/architecture.md`.
public enum AgentState: String, Sendable, CaseIterable {
  case working
  case blocked
  case idle
  case done
  case unknown
}

extension AgentState {
  /// Reads a backend's spelling of a state, treating anything unrecognized as `unknown`.
  ///
  /// Backends are free to grow states we have never heard of - herdr's API is explicitly
  /// unstable and ships weekly. Failing closed onto `unknown` means a Muster running
  /// against a newer daemon shows an honest "we don't know" instead of crashing or, far
  /// worse, quietly reading a novel state as `idle` and telling the user nothing needs
  /// them.
  public init(backendValue: String) {
    self = AgentState(rawValue: backendValue) ?? .unknown
  }
}
