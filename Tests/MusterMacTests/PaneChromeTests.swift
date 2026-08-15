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
      paneID: "w1:p1", zoomed: false, health: "stale",
      detail: "the daemon closed the connection")
    #expect(title.contains("w1:p1"))
    // The failure this exists to prevent: a window rendering an hour-old session as though
    // it were live, which is indistinguishable from a working one without saying so.
    #expect(title.contains("stale"))
  }

  @Test("a title says which daemon went stale")
  func staleNamesItsDaemon() {
    // A window showing a laptop and a devenv has two answers and one title bar. Saying only
    // "stale" leaves the reader to guess whether their local shell or their VPN is the
    // problem, and those are very different next actions.
    let title = PaneAppearance.title(
      paneID: "w1:p1", zoomed: false, health: "stale", detail: "the connection dropped",
      daemon: "devenv")
    #expect(title.contains("stale devenv"))
    #expect(title.contains("the connection dropped"))
  }

  @Test("a healthy window says nothing about its health")
  func connectedIsQuiet() {
    let title = PaneAppearance.title(
      paneID: "w1:p1", zoomed: false, health: "connected", detail: "")
    #expect(title == "muster - w1:p1")
  }

  @Test("a zoomed tab says so, because it looks like a tab with one pane")
  func zoomIsAdmitted() {
    // The other panes are still there and still running. Without this the user has no way to
    // learn why they vanished, and nothing to undo.
    let title = PaneAppearance.title(paneID: "w1:p1", zoomed: true, health: "connected", detail: "")
    #expect(title.contains("zoomed"))
  }

  @MainActor
  @Test("a state for another pane is not this pane's state")
  func otherPanesAreIgnored() {
    let chrome = pane()
    chrome.attach(paneID: "w1:p1")

    chrome.apply(paneID: "w1:p2", state: "blocked")
    #expect(chrome.state == "unknown")

    chrome.apply(paneID: "w1:p1", state: "blocked")
    #expect(chrome.state == "blocked")
  }

  @MainActor
  @Test("the keyboard and the agent are two different edges")
  func focusAndStateDoNotShareABorder() {
    // One edge carrying both would make a working pane and the focused pane
    // indistinguishable, which is exactly the confusion these are for clearing up. Fifteen
    // panes make that difference the whole product.
    let chrome = pane()
    chrome.attach(paneID: "w1:p1")

    chrome.apply(paneID: "w1:p1", state: "working")
    chrome.apply(focused: false)
    #expect(chrome.isFocused == false)
    #expect(chrome.state == "working")

    chrome.apply(focused: true)
    #expect(chrome.isFocused)
    #expect(chrome.state == "working")
  }

  @MainActor
  @Test("a click asks for the keyboard, and names the pane it is about")
  func aClickCarriesThePane() {
    let chrome = pane()
    chrome.attach(paneID: "w1:p3")
    var asked: [String] = []
    chrome.onFocusRequested = { asked.append($0) }

    chrome.surface.mouseDown(
      with: NSEvent.mouseEvent(
        with: .leftMouseDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
        context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!)

    #expect(asked == ["w1:p3"])
  }

  @Test("the renderer check has no pane and says so")
  func rendererCheckIsLabeled() {
    #expect(
      PaneAppearance.title(paneID: nil, zoomed: false, health: "disconnected", detail: "")
        .contains("renderer check"))
  }
}

@MainActor
private func pane() -> PaneChrome {
  let frame = NSRect(x: 0, y: 0, width: 100, height: 100)
  return PaneChrome(frame: frame, surface: SurfaceView(frame: frame))
}
