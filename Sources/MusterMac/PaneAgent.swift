import Foundation

/// One pane's agent as the core paints it: what it is doing, and what else is known of it.
///
/// Arrives on the same message as the state, and for the same reason: the roster is stable and
/// this blinks. An agent reports how full its context is as often as its statusline redraws.
///
/// Expressible as its state alone, which is all most of what reads it wants and all a pane the
/// core has said nothing else about has.
public struct PaneAgent: Equatable, Sendable, ExpressibleByStringLiteral {
  /// What a program in the pane says of its progress (OSC 9;4).
  public struct Progress: Equatable, Sendable {
    /// `running`, `error`, `indeterminate` or `paused`.
    public var state: String
    /// From 0 to 100, when the program said.
    public var percent: Int?
  }

  /// `working`, `blocked`, `waiting`, `idle`, `done` or `unknown`.
  public var state: String
  /// Whether the state is the agent's own report rather than what Muster read off its screen.
  public var reported = false
  /// Whether Muster's rules have stopped reading this agent's screen.
  public var unreadable = false
  /// How much of its context window is in use, from 0 to 100, when it has said.
  public var contextUsed: Float?
  public var subagents = 0
  public var model = ""
  public var costUSD: Double?
  /// What it ended its turn to wait on, empty when it is waiting on nothing.
  public var waiting = ""
  public var progress: Progress?
  /// A program in the pane rang the bell and nobody has looked at the pane since.
  public var rang = false
  /// `reporting`, `silent` (an adapter Muster ships is not reporting), `none` (its harness has
  /// no adapter), or empty while there is nothing to say yet.
  public var adapter = ""

  public init(state: String) {
    self.state = state
  }

  public init(stringLiteral state: String) {
    self.init(state: state)
  }

  init(_ changed: Muster_PaneStateChanged) {
    self.init(state: changed.state)
    reported = changed.reported
    unreadable = changed.unreadable
    rang = changed.rang
    adapter = changed.adapter
    if changed.hasFacts {
      let facts = changed.facts
      contextUsed = facts.hasContextUsed ? facts.contextUsed : nil
      subagents = Int(facts.subagents)
      model = facts.model
      costUSD = facts.hasCostUsd ? facts.costUsd : nil
      waiting = facts.waiting
    }
    if changed.hasProgress {
      progress = Progress(
        state: changed.progress.state,
        percent: changed.progress.hasPercent ? Int(changed.progress.percent) : nil)
    }
  }
}
