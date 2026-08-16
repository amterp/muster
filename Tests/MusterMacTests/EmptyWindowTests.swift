import Testing

@testable import MusterMac

// A window with no panes used to be a window nobody could refill: every action is about a
// pane, there was none, and nothing on screen said which key was the exception. What is
// pinned here is that the way out is named, and named with the chord this window is actually
// bound to rather than the one that shipped.

@MainActor
@Suite("empty window")
struct EmptyWindowTests {
  @Test("the way out is spelled with the chord that is bound")
  func hintFollowsTheBinding() {
    let message = EmptyWindow.message(bindings: [
      Core.Binding(action: "new_tab", key: "KeyT", modifiers: ["super"])
    ])
    #expect(message.hint == "Press ⌘T to open one.")
  }

  @Test("a rebound chord is the one offered")
  func hintFollowsARebind() {
    // The reason this reads the bindings at all. A hint naming ⌘T in a window where ⌘T is
    // somebody's own chord for something else is worse than no hint.
    let message = EmptyWindow.message(bindings: [
      Core.Binding(action: "new_tab", key: "KeyN", modifiers: ["super", "shift"])
    ])
    #expect(message.hint == "Press ⇧⌘N to open one.")
  }

  @Test("an unbound action is still reachable, and the hint says how")
  func unboundPointsAtTheMenu() {
    // Unbinding is supported, so this is a real window rather than a defensive branch. The
    // menu item is still there and still works, and naming it is the only true instruction.
    let message = EmptyWindow.message(bindings: [
      Core.Binding(action: "new_tab", key: "", modifiers: [])
    ])
    #expect(message.hint == "Open one from New Tab, in the Tab menu.")
  }

  @Test("a core that named no bindings still explains itself")
  func nothingBoundStillPointsAtTheMenu() {
    // `Core.bindings()` answers empty when the core will not answer at all, which is a
    // window somebody can still click around - so the hint has to survive it.
    #expect(EmptyWindow.message(bindings: []).hint.contains("New Tab"))
  }

  @Test("the headline says what happened, not what is wrong")
  func headlineIsNotAnError() {
    // Closing your last pane is an ordinary thing to do. Wording it as a failure would send
    // somebody looking for a bug in a window that is working.
    #expect(EmptyWindow.message(bindings: []).headline == "No panes open.")
  }
}
