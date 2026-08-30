import AppKit
import Testing

@testable import MusterMac

// When a half-typed numbered chord is over. Only `tab_then_pane` has anything to decide, and
// the whole of the decision is made from the keyboard - which is why it is pinned here rather
// than in the corpus: the core is handed requests and never sees a modifier.
//
// The rebound cases are the reason this is a test rather than something to try in the app.
// Nobody driving Muster would think to put the nine chords on ctrl, and a version of this that
// hardcoded ⌘ would work perfectly for everybody who never did.

@Suite("numbered chord")
struct NumberedChordTests {
  @Test("letting go of the modifier ends a chord that was still being typed")
  func releasingEndsIt() {
    let chord: NSEvent.ModifierFlags = [.command]
    #expect(NumberedChord.ends(numbering: .panesInTab, held: [], chord: chord))
    // Still down, so nothing has been decided yet - this is the moment the second press is
    // being chosen, and it is the whole reason the badges are on screen.
    #expect(NumberedChord.ends(numbering: .panesInTab, held: [.command], chord: chord) == false)
    // Reaching for ⇧ mid-gesture is not letting go of it.
    #expect(
      NumberedChord.ends(numbering: .panesInTab, held: [.command, .shift], chord: chord) == false)
  }

  @Test("a window with no chord half-typed says nothing, whatever the hand does")
  func idleWindowsStaySilent() {
    // ⌘ comes up dozens of times a minute for ⌘C, ⌘T, ⌘W. None of them is a gesture ending,
    // and a window that reported them would republish its agent list for every copy.
    #expect(NumberedChord.ends(numbering: .panes, held: [], chord: [.command]) == false)
    #expect(NumberedChord.ends(numbering: .tabs, held: [], chord: [.command]) == false)
  }

  @Test("the modifier watched for is the one the nine are actually bound with")
  func reboundChordsMoveTheModifier() {
    let rebound = (1...9).map {
      Core.Binding(action: "focus_pane_\($0)", key: "\($0)", modifiers: ["control"])
    }
    #expect(NumberedChord.modifiers(rebound) == [.control])
    // And the gesture ends on that one rather than on ⌘, which is never held here at all.
    #expect(NumberedChord.ends(numbering: .panesInTab, held: [], chord: [.control]))
    #expect(
      NumberedChord.ends(numbering: .panesInTab, held: [.control], chord: [.control]) == false)
  }

  @Test("the shared modifier is what every numbered chord needs, not what any of them uses")
  func theMaskIsAnIntersection() {
    // A file that moved one of the nine onto ⌘⇧ and left the rest on ⌘. Releasing ⇧ has not
    // ended anything, because eight of the nine presses that could come next do not want it.
    var bindings = (1...8).map {
      Core.Binding(action: "focus_pane_\($0)", key: "\($0)", modifiers: ["super"])
    }
    bindings.append(Core.Binding(action: "focus_pane_9", key: "9", modifiers: ["super", "shift"]))
    #expect(NumberedChord.modifiers(bindings) == [.command])
  }

  @Test("nothing shared means nothing to let go of, and the chord ends the way it always has")
  func noSharedModifierNeverFires() {
    // Unbound, so no press can arm one in the first place.
    #expect(NumberedChord.modifiers([]) == [])
    // Bound to bare digits. There is no modifier being held, so there is none to release, and
    // an empty mask must not read as "you are holding nothing, so you have let go".
    let bare = (1...9).map { Core.Binding(action: "focus_pane_\($0)", key: "\($0)", modifiers: []) }
    #expect(NumberedChord.modifiers(bare) == [])
    #expect(NumberedChord.ends(numbering: .panesInTab, held: [], chord: []) == false)
  }

  @Test("only the numbered actions decide it")
  func otherActionsAreIgnored() {
    let bindings = [
      Core.Binding(action: "split_right", key: "d", modifiers: ["super"]),
      Core.Binding(action: "zoom", key: "return", modifiers: ["super", "shift"]),
      Core.Binding(action: "focus_pane_1", key: "1", modifiers: ["control", "alt"]),
    ]
    #expect(NumberedChord.modifiers(bindings) == [.control, .option])
  }

  @Test("an action somebody unbound cannot narrow the modifier to nothing")
  func unboundActionsAreSkipped() {
    // An empty key is how the core spells an action with no chord. Counted as a binding with
    // no modifiers, it would empty the intersection and quietly turn the whole feature off.
    var bindings = (1...8).map {
      Core.Binding(action: "focus_pane_\($0)", key: "\($0)", modifiers: ["super"])
    }
    bindings.append(Core.Binding(action: "focus_pane_9", key: "", modifiers: []))
    #expect(NumberedChord.modifiers(bindings) == [.command])
  }
}

// The routing, separately from the decision. AppKit sends a modifier event to the first
// responder and every responder above it, and Muster's first responder is whichever pane,
// list or find field was last clicked - so the claim being pinned is that nothing in the view
// stack swallows one on its way up. It is the assumption the whole feature rests on and the
// one that cannot be read off the code: `NSResponder`'s default forwards, and an override
// anywhere below here would stop it silently.

@Suite("modifier events reach the window")
@MainActor
struct ModifierRoutingTests {
  @Test("a modifier released over a pane reaches the window above it")
  func modifiersTravelUpFromAPane() {
    let window = KeyboardWindow(
      contentRect: NSRect(x: 0, y: 0, width: 400, height: 300),
      styleMask: [.titled], backing: .buffered, defer: false)
    let frame = NSRect(x: 0, y: 0, width: 200, height: 200)
    let chrome = PaneChrome(frame: frame, surface: SurfaceView(frame: frame))
    window.contentView?.addSubview(chrome)

    var held: [NSEvent.ModifierFlags] = []
    window.onModifiersChanged = { held.append($0) }

    // Sent to the surface, which is where AppKit would send it: a pane is what has the
    // keyboard almost all the time.
    chrome.surface.flagsChanged(with: modifiers([.command]))
    chrome.surface.flagsChanged(with: modifiers([]))

    #expect(held == [[.command], []])
  }

  @Test("a modifier released with the agent list focused reaches it too")
  func modifiersTravelUpFromTheSidebar() {
    // The other place the keyboard sits. A gesture begun with ⌘2 and ended after clicking a
    // row has to end, and the two responder chains are different ones.
    let window = KeyboardWindow(
      contentRect: NSRect(x: 0, y: 0, width: 400, height: 300),
      styleMask: [.titled], backing: .buffered, defer: false)
    let sidebar = SidebarView(frame: NSRect(x: 0, y: 0, width: 200, height: 300))
    window.contentView?.addSubview(sidebar)

    var held: [NSEvent.ModifierFlags] = []
    window.onModifiersChanged = { held.append($0) }
    sidebar.flagsChanged(with: modifiers([]))

    #expect(held == [[]])
  }
}

private func modifiers(_ flags: NSEvent.ModifierFlags) -> NSEvent {
  NSEvent.keyEvent(
    with: .flagsChanged, location: .zero, modifierFlags: flags, timestamp: 0, windowNumber: 0,
    context: nil, characters: "", charactersIgnoringModifiers: "", isARepeat: false,
    keyCode: 55)!
}
