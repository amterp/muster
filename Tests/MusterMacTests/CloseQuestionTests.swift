import Testing

@testable import MusterMac

@Suite("which closes ask first")
@MainActor
struct CloseQuestionTests {
  private static func pane(_ id: String, agent: String = "", label: String = "") -> Roster.Pane {
    Roster.Pane(
      key: PaneKey(daemon: "laptop", pane: id), label: label.isEmpty ? id : label,
      onScreen: true, agent: agent)
  }

  private static func tab(_ panes: [Roster.Pane]) -> Roster.Tab {
    Roster.Tab(id: "t1", place: 1, label: "work", onScreen: true, panes: panes)
  }

  @Test("a pane running an agent asks, and names the pane")
  func anAgentPaneAsks() {
    let question = CloseQuestion.pane(Self.pane("p1", agent: "claude", label: "muster · claude"))
    #expect(question?.heading == "Close muster · claude?")
    #expect(question?.confirm == "Close Pane")
  }

  @Test("a plain shell closes at once")
  func aShellDoesNotAsk() {
    #expect(CloseQuestion.pane(Self.pane("p1")) == nil)
  }

  @Test("a pane the roster has not listed closes at once rather than not at all")
  func anUnknownPaneDoesNotAsk() {
    #expect(CloseQuestion.pane(nil) == nil)
    #expect(CloseQuestion.tab(nil) == nil)
  }

  @Test("a tab asks when any one of its panes runs an agent, and counts them")
  func aTabAsksOnItsAgents() {
    let mixed = Self.tab([
      Self.pane("p1"), Self.pane("p2", agent: "claude"), Self.pane("p3", agent: "codex"),
    ])
    let question = CloseQuestion.tab(mixed)
    #expect(question?.heading == "Close work?")
    #expect(question?.confirm == "Close Tab")
    #expect(question?.body.contains("3 panes") == true)
    #expect(question?.body.contains("2 agents") == true)

    let single = CloseQuestion.tab(Self.tab([Self.pane("p1", agent: "claude")]))
    #expect(single?.body.contains("its pane") == true)
    #expect(single?.body.contains("the agent running in it") == true)
  }

  @Test("a tab of shells closes at once")
  func aTabOfShellsDoesNotAsk() {
    #expect(CloseQuestion.tab(Self.tab([Self.pane("p1"), Self.pane("p2")])) == nil)
    #expect(CloseQuestion.tab(Self.tab([])) == nil)
  }
}

@Suite("which keystroke confirms a sheet")
@MainActor
struct ConfirmKeyTests {
  @Test("a question takes Return only when it says so, and a tell always does")
  func returnIsPerQuestion() {
    #expect(ConfirmSheet.confirmKey(confirmOnReturn: false, cancellable: true) == "")
    #expect(ConfirmSheet.confirmKey(confirmOnReturn: true, cancellable: true) == "\r")
    #expect(ConfirmSheet.confirmKey(confirmOnReturn: false, cancellable: false) == "\r")
  }
}
