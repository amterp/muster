import Testing

@testable import MusterMac

@MainActor @Suite("the Hidden Panes menu")
struct HiddenPanesMenuTests {
  /// The menu reaches hidden panes while the sidebar is closed, so each is named as its row in
  /// the Hidden section is, with its agent's state where there is one worth saying.
  @Test("lists every hidden pane by its row's label, with its agent's state")
  func listsHiddenPanes() {
    let worker = PaneKey(daemon: "laptop", pane: "p1")
    let shell = PaneKey(daemon: "devenv", pane: "p2")
    let shown = PaneKey(daemon: "laptop", pane: "p3")
    let roster = Roster(
      tabs: [
        Roster.Tab(
          id: "t1", place: 1, label: "main", onScreen: true,
          panes: [Roster.Pane(key: shown, label: "editor", onScreen: true)])
      ],
      hidden: [
        Roster.Tab(
          id: "t2", place: 0, label: "", onScreen: false,
          panes: [
            Roster.Pane(key: worker, label: "otter · claude", onScreen: false),
            Roster.Pane(key: shell, label: "zsh", onScreen: false),
          ])
      ])

    let entries = HiddenPanesMenu.entries(
      roster: roster, states: [worker: "blocked", shell: "unknown", shown: "working"])

    #expect(
      entries == [
        .init(pane: worker, title: "otter · claude · blocked"),
        .init(pane: shell, title: "zsh"),
      ])
  }
}
