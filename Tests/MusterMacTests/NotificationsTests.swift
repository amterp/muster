import Testing

@testable import MusterMac

// The words a banner uses, which is the shell's whole contribution to attention routing -
// everything about *whether* to post one is decided in the core and pinned in
// corpus/conformance/attention-notifying.json. What is left here is small and worth pinning
// anyway: a banner is often all somebody sees of an agent that needs them, and it is the one
// surface in Muster with no second line to correct a first.

@Suite("what a notification about a pane says")
struct NotificationsTests {
  @Test("the title is what the agent list calls the pane")
  func titleIsTheRosterLabel() {
    // The same string the row carries, because the person reading this is about to go and
    // look for that row. Two names for one agent is the failure worth spending a test on.
    #expect(
      PaneNotification.title(label: "🔥 payments spike", paneID: "p1w3r07bsd")
        == "🔥 payments spike")
  }

  @Test("a pane the core could not name still says which pane it is")
  func titleFallsBackToTheID() {
    // A pane the mirror had already let go of by the time the notification was built. Rare,
    // and an id somebody can type into `muster focus` beats a blank banner.
    #expect(PaneNotification.title(label: "", paneID: "p1w3r07bsd") == "p1w3r07bsd")
  }

  @Test("the reason distinguishes being waited on from having finished")
  func reasonSaysWhich() {
    // The two are opposite calls on the reader's time - one is somebody held up right now -
    // so a banner that read the same for both would be a banner nobody could triage.
    #expect(PaneNotification.reason(state: "blocked") == "is waiting on you")
    #expect(PaneNotification.reason(state: "done") == "has finished")
    #expect(PaneNotification.reason(state: "blocked") != PaneNotification.reason(state: "done"))
  }

  @Test("a state the core never raises still says something true")
  func anUnknownStateIsNotBlank() {
    // The core raises `blocked` and `done` and nothing else, so this is a seam disagreement.
    // A banner is the wrong place to report one, and an empty body is worse than a vague one.
    #expect(!PaneNotification.reason(state: "compacting").isEmpty)
  }

  @Test("a banner is identified by machine and pane, not pane alone")
  func identifierNamesTheMachine() {
    // A window showing a laptop and a devenv holds two panes called w1:p1. Keyed by pane
    // alone, one machine's agent going quiet would take down the other machine's banner -
    // and posting the second would silently replace the first.
    #expect(
      PaneNotification.identifier(daemon: "local", pane: "w1:p1")
        != PaneNotification.identifier(daemon: "devenv", pane: "w1:p1"))
  }
}
