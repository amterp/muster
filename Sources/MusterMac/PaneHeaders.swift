import Foundation

/// Hands every pane's header the row it should draw, and only when that row changed.
///
/// The roster and the agents arrive as two messages in either order, so this keeps both and joins
/// them as the list does - which is why a header's row comes from the same builder as the list's
/// (`SidebarModel.paneRow`), and the two cannot disagree about what a pane's line says. A state
/// change reaches one header; a roster republish that changed nothing reaches none, because a
/// header ignores a row equal to the one it shows.
@MainActor
final class PaneHeaders {
  /// The time a state's age is measured against. Replaceable so that a test can let a minute pass.
  var clock: () -> Date = { Date() }

  /// Called after a refresh, for whoever keeps the minute counted.
  var onShowsTimeChanged: (() -> Void)?

  private let chrome: (PaneKey) -> PaneChrome?
  private var panes: [PaneKey: (pane: Roster.Pane, tab: Roster.Tab)] = [:]
  private var agents: [PaneKey: PaneAgent] = [:]

  init(chrome: @escaping (PaneKey) -> PaneChrome?) {
    self.chrome = chrome
  }

  /// Whether some header shows how long a state has held.
  var showsTime: Bool {
    panes.keys.contains { SidebarModel.showsTime(of: agents[$0] ?? "unknown") }
  }

  /// A roster arrived, or the agents did: every pane's header is checked against the new inputs.
  func apply(roster: Roster, agents: [PaneKey: PaneAgent]) {
    self.agents = agents
    panes = [:]
    for tab in roster.tabs {
      for pane in tab.panes { panes[pane.key] = (pane, tab) }
    }
    refreshAll()
  }

  /// One agent changed what it is doing.
  func apply(pane key: PaneKey, agent: PaneAgent) {
    agents[key] = agent
    refresh(key)
    onShowsTimeChanged?()
  }

  /// A minute passed: only the headers showing a time come out different.
  func tick() {
    refreshAll()
  }

  func refreshAll() {
    for key in panes.keys { refresh(key, notifying: false) }
    onShowsTimeChanged?()
  }

  /// Checks one pane's header, for a chrome that has just been made.
  func refresh(_ key: PaneKey) {
    refresh(key, notifying: true)
  }

  private func refresh(_ key: PaneKey, notifying: Bool) {
    guard let chrome = chrome(key) else { return }
    guard let held = panes[key] else {
      chrome.apply(header: nil)
      return
    }
    chrome.apply(
      header: SidebarModel.paneRow(
        held.pane, in: held.tab.id, agent: agents[key] ?? "unknown", now: clock(),
        presses: (held.pane.tabPress, held.pane.press), onScreen: held.pane.onScreen))
    if notifying { onShowsTimeChanged?() }
  }
}
