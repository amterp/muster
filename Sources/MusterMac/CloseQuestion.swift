import Foundation

/// What closing a pane or a tab asks first, or nothing when it should just close.
///
/// A pane that runs an agent asks, because closing it ends the agent mid-task; a plain shell does
/// not, so `cmd+w` stays as quick as it was where there is nothing to lose. "Runs an agent" is the
/// harness the pane's daemon detected (`Roster.Pane.agent`), the fact the row's label is already
/// built from. A pane the roster has not listed yet closes without a question: the roster is the
/// only place the fact is held, and a window that refused to close what it cannot yet describe
/// would be worse than one that closes a pane it was slow to learn about.
///
/// Pure, and apart from the sheet that shows it, like `QuitSummary`: which closes ask is a decision
/// worth cases of its own, and a sheet needs a window.
struct CloseQuestion: Equatable {
  let heading: String
  let body: String
  let confirm: String

  static func pane(_ pane: Roster.Pane?) -> CloseQuestion? {
    guard let pane, !pane.agent.isEmpty else { return nil }
    return CloseQuestion(
      heading: "Close \(pane.label)?",
      body: "Ends the agent running in it, interrupting whatever it is doing. Reopen Pane brings "
        + "it back in its session.",
      confirm: "Close Pane")
  }

  /// A tab asks when any pane it holds runs an agent, and says how many panes and agents that is.
  static func tab(_ tab: Roster.Tab?) -> CloseQuestion? {
    guard let tab else { return nil }
    let agents = tab.panes.filter { !$0.agent.isEmpty }.count
    guard agents > 0 else { return nil }
    let panes = tab.panes.count
    let what = panes == 1 ? "its pane" : "its \(panes) panes"
    let who = agents == 1 ? "the agent" : "the \(agents) agents"
    let within = panes == 1 ? "it" : "them"
    let doing = agents == 1 ? "it is" : "they are"
    return CloseQuestion(
      heading: "Close \(tab.label)?",
      body: "Closes \(what) and ends \(who) running in \(within), interrupting whatever "
        + "\(doing) doing.",
      confirm: "Close Tab")
  }
}
