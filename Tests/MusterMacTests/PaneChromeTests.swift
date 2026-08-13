import AppKit
import Testing

@testable import MusterMac

// The window's whole contribution to "agent states are the point": a border and a title.
// Small, and worth pinning anyway - the core sends every pane's transitions, so a window
// that forgets to filter shows its neighbor's agent as its own, and that failure is
// invisible until there are two panes to confuse.

@Suite("pane chrome")
struct PaneChromeTests {
  @Test("an unrecognized state is unknown, never idle")
  func unknownStateIsNotIdle() {
    // Not the same assertion twice: the point is that a state herdr invents next week
    // gets the resting appearance of something unreadable rather than being colored as an
    // agent that finished. A user who learns the colors lie stops reading them.
    #expect(PaneAppearance.isHighlighted(state: "compacting") == false)
    #expect(
      PaneAppearance.borderColor(state: "compacting")
        == PaneAppearance.borderColor(state: "unknown"))
    #expect(
      PaneAppearance.borderColor(state: "compacting") != PaneAppearance.borderColor(state: "done"))
  }

  @Test("only the states worth noticing get a border")
  func restingStatesAreBare() {
    #expect(PaneAppearance.isHighlighted(state: "working"))
    #expect(PaneAppearance.isHighlighted(state: "blocked"))
    #expect(PaneAppearance.isHighlighted(state: "done"))
    // Every pane carrying a border all the time is every pane carrying none.
    #expect(PaneAppearance.isHighlighted(state: "idle") == false)
    #expect(PaneAppearance.isHighlighted(state: "unknown") == false)
  }

  @Test("a stale backend is admitted in the title")
  func staleIsVisible() {
    let title = PaneAppearance.title(
      paneID: "w1:p1", state: "working", health: "stale", detail: "the daemon closed the connection"
    )
    #expect(title.contains("w1:p1"))
    #expect(title.contains("working"))
    // The failure this exists to prevent: a window rendering an hour-old session as though
    // it were live, which is indistinguishable from a working one without saying so.
    #expect(title.contains("stale"))
  }

  @Test("a healthy window says nothing about its health")
  func connectedIsQuiet() {
    let title = PaneAppearance.title(
      paneID: "w1:p1", state: "idle", health: "connected", detail: "")
    #expect(title == "muster - w1:p1")
  }

  @MainActor
  @Test("a state for another pane is not this pane's state")
  func otherPanesAreIgnored() {
    let chrome = PaneChrome(
      frame: NSRect(x: 0, y: 0, width: 100, height: 100),
      surface: SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100)))
    chrome.attach(paneID: "w1:p1")

    chrome.apply(paneID: "w1:p2", state: "blocked")
    #expect(chrome.state == "unknown")

    chrome.apply(paneID: "w1:p1", state: "blocked")
    #expect(chrome.state == "blocked")
  }

  @MainActor
  @Test("the renderer check has no pane and says so")
  func rendererCheckIsLabeled() {
    let chrome = PaneChrome(
      frame: NSRect(x: 0, y: 0, width: 100, height: 100),
      surface: SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100)))
    chrome.attach(paneID: nil)
    #expect(
      PaneAppearance.title(
        paneID: chrome.paneID, state: chrome.state, health: chrome.health, detail: ""
      )
      .contains("renderer check"))
  }
}
