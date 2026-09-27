import AppKit
import Testing

@testable import MusterMac

// The bug that started all of this - every printable key reaching the pane twice - lived
// in this view, in an executable no test could import. The core's conformance corpus pins
// the decision; this pins the wiring, which is the other half: a decision made correctly
// and then called wrongly looks identical from the outside.
//
// No NSApplication, no window, no surface, and no core. A view and a keystroke, with the
// seam recorded rather than crossed - so what these assert on is the request that would
// have gone to the core, which is the daemon-facing oracle one layer up (docs/testing.md).

@MainActor
private func view(_ recorder: RecordingDispatcher) -> SurfaceView {
  seam(recorder)
  let surface = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
  surface.attach(typeable: true)
  return surface
}

@Test(.ownsTheSeam) @MainActor func aTypedCharacterCrossesTheSeamExactlyOnce() {
  let recorder = RecordingDispatcher()
  view(recorder).keyDown(with: key("h", keyCode: 0x04))

  // Once. The regression is two.
  #expect(recorder.requests.count == 1)
  #expect(recorder.requests.first?.keyDown.key.key == "KeyH")
  #expect(recorder.requests.first?.keyDown.key.text == "h")
  #expect(recorder.requests.first?.keyDown.key.action == "press")
}

@Test(.ownsTheSeam) @MainActor func typingAWordSendsThatWordAndNoMore() {
  // `hello` became `hheelllloo`, so spell it out: the failure was only visible in
  // aggregate, and a single-character case can pass while this one fails.
  let recorder = RecordingDispatcher()
  let surface = view(recorder)
  for (character, code) in [("h", 0x04), ("e", 0x0e), ("l", 0x25), ("l", 0x25), ("o", 0x1f)] {
    surface.keyDown(with: key(character, keyCode: UInt16(code)))
  }

  #expect(recorder.requests.map { $0.keyDown.key.text }.joined() == "hello")
}

@Test(.ownsTheSeam) @MainActor func aKeyPressCarriesTheCompositionSignalsRatherThanResolvingThem() {
  // The dead-key shape: a preedit is open and the next press resolves it. What the pane
  // must receive is the composed character rather than the key that finished it - and the
  // shell's job is to report all three signals, not to pick between them.
  let recorder = RecordingDispatcher()
  let surface = view(recorder)
  surface.setMarkedText(
    "´", selectedRange: NSRange(location: 0, length: 1), replacementRange: NSRange())

  surface.keyDown(with: key("e", keyCode: 0x0e))

  #expect(recorder.requests.count == 1)
  let down = recorder.requests[0].keyDown
  #expect(down.wasComposing)
  #expect(down.committed == "e")
  #expect(!down.stillComposing)
  #expect(!surface.hasMarkedText())
}

@Test(.ownsTheSeam) @MainActor func committedTextFromOutsideAKeystrokeIsStillSent() {
  // A character picker or a service commits text with no key press behind it. Nothing else
  // is going to send that, so the view must.
  let recorder = RecordingDispatcher()
  view(recorder).insertText("→", replacementRange: NSRange())

  #expect(recorder.requests.map { $0.sendText.text } == ["→"])
}

@Test(.ownsTheSeam) @MainActor func aWheelScrollsTheSurfaceAndIsReportedAsItWasGiven() {
  // Two halves of one gesture, and they have to agree. The surface scrolls its own history, or
  // answers as the pane's modes say; the daemon decides what the program gets. Handed different
  // numbers, the two would scroll different distances. Unscaled: `scroll_multiplier` is the
  // core's to apply, and a shell that applied it too would scale the program's scroll twice.
  //
  // Reported rather than sent, because a wheel is addressed to a pane and this view does not
  // know which one it is showing. Which pane it names is pinned a layer up, where the id is.
  let recording = RecordingSurface()
  let surface = view(surface: recording, clipboard: NSPasteboard.general)
  var reported: [Core.Wheel] = []
  surface.onPointer = { if case .wheel(let wheel) = $0 { reported.append(wheel) } }
  guard let event = wheel(deltaY: 3, modifiers: .maskAlternate) else { return }

  surface.scrollWheel(with: event)

  #expect(recording.scrolls.map(\.dy) == [3])
  #expect(recording.scrolls.map(\.precise) == [false])
  #expect(reported.map(\.dy) == [3])
  #expect(reported.map(\.precise) == [false])
  #expect(reported.map(\.modifiers) == [["alt"]])
}

@Test(.ownsTheSeam) @MainActor func aWheelIsPlacedInThePanesPixelsFromItsTopLeft() {
  // The daemon compares the pointer against the terminal's size, which it knows in backing
  // pixels from the top left. Points from the bottom left would put a click on the mirror-image
  // row at half the distance.
  let surface = view(surface: RecordingSurface(), clipboard: NSPasteboard.general)
  var reported: [Core.Wheel] = []
  surface.onPointer = { if case .wheel(let wheel) = $0 { reported.append(wheel) } }
  guard let event = wheel(deltaY: 3, at: NSPoint(x: 30, y: 20)) else { return }

  surface.scrollWheel(with: event)

  // No window, so the view falls back to the 2x it assumes everywhere else it needs a scale,
  // and the event's position is its own: where CoreGraphics puts it depends on the screen.
  let point = event.locationInWindow
  #expect(reported.map(\.x) == [Double(point.x * 2)])
  #expect(reported.map(\.y) == [Double((100 - point.y) * 2)])
}

@Test(.ownsTheSeam) @MainActor func theRendererCheckScrollsWithNoDaemonToTell() {
  // A bare `muster` runs a shell straight in its surface, whose history is the only one there
  // is. Nothing in it is a daemon's pane, so there is nobody to report the wheel to.
  let recording = RecordingSurface()
  let surface = view(surface: recording, clipboard: NSPasteboard.general)
  surface.attach(typeable: false)
  var reported: [Core.Wheel] = []
  surface.onPointer = { if case .wheel(let wheel) = $0 { reported.append(wheel) } }
  guard let event = wheel(deltaY: 3) else { return }

  surface.scrollWheel(with: event)

  #expect(recording.scrolls.count == 1)
  #expect(reported.isEmpty)
}

@Test(.ownsTheSeam) @MainActor func aCellIsReportedInPointsRatherThanBackingPixels() {
  // libghostty measures in backing pixels and every dimension a config file names is points,
  // so somebody who wrote `resize_step = "16px"` on a retina display means two cells here, not
  // one. Converted in the view because that is where AppKit keeps the scale factor.
  let recording = RecordingSurface()
  recording.cellPixelSize = (width: 16, height: 34)
  let surface = view(surface: recording, clipboard: NSPasteboard.general)

  // No window, so the view falls back to the 2x it assumes everywhere else it needs a scale.
  let cell = surface.cellPointSize

  #expect(cell?.width == 8)
  #expect(cell?.height == 17)
}

@Test(.ownsTheSeam) @MainActor func aSurfaceNothingHasSizedYetReportsNoCellRatherThanZero() {
  // Zero would reach the core as a cell of no width and be divided by. Nil says "could not
  // measure", which the core answers with the daemon's own step.
  let surface = view(surface: RecordingSurface(), clipboard: NSPasteboard.general)

  #expect(surface.cellPointSize == nil)
}

@Test(.ownsTheSeam) @MainActor func aWheelOverAPaneNeverAsksForTheKeyboard() {
  // The whole point of the feature: reading one agent's output while typing into another. A
  // scroll that also focused would make that impossible in exactly the case it exists for.
  let surface = view(recorder())
  var focused = false
  surface.onClick = { focused = true }
  guard let event = wheel(deltaY: 3) else { return }

  surface.scrollWheel(with: event)

  #expect(focused == false)
}

@Test(.ownsTheSeam) @MainActor func aViewWithNoPaneSendsNothingRatherThanRefusalsPerKeystroke() {
  // A bare `muster` is the renderer check, and every key it swallows is expected. Sending
  // them anyway would fill the log with a refusal per keystroke for a state that is normal.
  let recorder = RecordingDispatcher()
  seam(recorder)
  let surface = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
  surface.attach(typeable: false)

  surface.keyDown(with: key("h", keyCode: 0x04))

  #expect(recorder.requests.isEmpty)
}

// The surface's half of a keystroke. The program's bytes go through the core to the daemon; the
// surface is handed the same key afterwards for what it does by itself - scroll to the bottom,
// clear a selection - and only when the program got it, so a chord Muster kept moves nothing.

/// A core that says whether each press reached the pane's program.
private func keysReachThePane(_ toPane: Bool) -> RecordingDispatcher {
  RecordingDispatcher { request in
    guard case .keyDown = request.payload else { return nil }
    var handled = Muster_KeyHandled()
    handled.toPane = toPane
    return .keyHandled(handled)
  }
}

@Test(.ownsTheSeam) @MainActor func aKeyTheProgramGotReachesTheSurface() {
  let recording = RecordingSurface()
  let pane = view(
    surface: recording, clipboard: NSPasteboard.general, recorder: keysReachThePane(true))

  pane.keyDown(with: key("h", keyCode: 0x04))

  #expect(recording.presses.map(\.keyCode) == [0x04])
  #expect(recording.presses.map(\.composing) == [false])
}

@Test(.ownsTheSeam) @MainActor func aKeyMusterKeptNeverReachesTheSurface() {
  // A key the program never saw scrolling the pane to the bottom, or clearing what somebody
  // had selected, would be the surface acting on a keystroke that was not the pane's.
  let recording = RecordingSurface()
  let pane = view(
    surface: recording, clipboard: NSPasteboard.general, recorder: keysReachThePane(false))

  pane.keyDown(with: key("h", keyCode: 0x04))

  #expect(recording.presses.isEmpty)
}

@Test(.ownsTheSeam) @MainActor func aKeyTheCoreDidNotAnswerNeverReachesTheSurface() {
  // A refusal or an answer of another kind is not a yes. `ok` is what the recorder says to
  // everything it was not told about.
  let recording = RecordingSurface()
  let pane = view(surface: recording, clipboard: NSPasteboard.general)

  pane.keyDown(with: key("h", keyCode: 0x04))

  #expect(recording.presses.isEmpty)
}

@Test(.ownsTheSeam) @MainActor func aReleaseAndAModifierReachTheSurface() {
  let recording = RecordingSurface()
  let pane = view(surface: recording, clipboard: NSPasteboard.general)

  pane.keyUp(with: release("h", keyCode: 0x04))
  pane.flagsChanged(with: modifiers([.command]))

  #expect(recording.releases == [0x04])
  #expect(recording.modifierChanges.map { $0.contains(.command) } == [true])
}

@Test(.ownsTheSeam) @MainActor func aViewWithNoPaneHandsItsSurfaceNoKeys() {
  // The renderer check runs a shell straight in its surface and swallows the keyboard, which
  // the title says. A surface handed keys anyway would type into that shell.
  let recording = RecordingSurface()
  let pane = view(
    surface: recording, clipboard: NSPasteboard.general, recorder: keysReachThePane(true))
  pane.attach(typeable: false)

  pane.keyDown(with: key("h", keyCode: 0x04))
  pane.keyUp(with: release("h", keyCode: 0x04))
  pane.flagsChanged(with: modifiers([.command]))

  #expect(recording.presses.isEmpty)
  #expect(recording.releases.isEmpty)
  #expect(recording.modifierChanges.isEmpty)
}

// Selection and the clipboard, which is the one input path that never reaches the core: the
// grid libghostty painted is where a drag lands, so the oracle here is the surface rather than
// the seam.

@MainActor
private func view(
  surface: RecordingSurface, clipboard: NSPasteboard,
  recorder: RecordingDispatcher = RecordingDispatcher()
) -> SurfaceView {
  seam(recorder)
  let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
  view.pasteboard = clipboard
  view.attach(surface, typeable: true)
  return view
}

/// A pasteboard nobody else is using, so a test neither reads nor destroys what the developer
/// running it last copied.
private func scratchClipboard(_ name: String) -> NSPasteboard {
  let board = NSPasteboard(name: NSPasteboard.Name("muster.tests.\(name)"))
  board.clearContents()
  return board
}

@Test(.ownsTheSeam) @MainActor func aDragArrivesInTheSurfacesOwnCoordinates() {
  // The y flip, which is the whole of what this view decides about a drag. Unflipped, a
  // selection is the mirror image of the one that was dragged - visible instantly in the app
  // and invisible in every green test, which is why it is asserted here.
  let surface = RecordingSurface()
  let pane = view(surface: surface, clipboard: scratchClipboard("drag"))

  pane.mouseDown(with: click(at: NSPoint(x: 10, y: 90)))
  pane.mouseDragged(with: drag(to: NSPoint(x: 50, y: 40)))
  pane.mouseUp(with: release(at: NSPoint(x: 50, y: 40)))

  #expect(
    surface.positions == [
      NSPoint(x: 10, y: 10), NSPoint(x: 50, y: 60), NSPoint(x: 50, y: 60),
    ])
  // Pressed, then released. A press with no release leaves the surface selecting forever.
  #expect(surface.buttons.map(\.pressed) == [true, false])
  #expect(surface.buttons.map(\.number) == [0, 0])
}

@Test(.ownsTheSeam) @MainActor func aClickIsMeasuredFromWhereTheTextStarts() {
  // The daemon's grid starts where the text does, inside the padding, as libghostty sizes it.
  // Measured from the view's edge, every click in vim with `pane_padding = 10` landed a column
  // right and half a row low.
  let recorder = RecordingDispatcher()
  let recording = RecordingSurface()
  recording.padding = 10
  recording.textPixelSize = (width: 150, height: 150)
  let pane = paneOnTheSeam(recording, recorder)

  pane.mouseDown(with: click(at: NSPoint(x: 10, y: 90)))
  // Over the padding, which is the nearest cell as far as the program is concerned: the top
  // left one, then the bottom right one.
  pane.mouseDragged(with: drag(to: NSPoint(x: 4, y: 96)))
  pane.mouseUp(with: release(at: NSPoint(x: 96, y: 4)))

  let sent = mice(recorder)
  #expect(sent.map(\.x) == [0, 0, 149])
  #expect(sent.map(\.y) == [0, 0, 149])
}

@Test(.ownsTheSeam) @MainActor func aPressCarriesThePointerBeforeTheButton() {
  // libghostty holds the pointer position separately from the button, so a press reported
  // without one starts the selection wherever the pointer was last seen - which after a click
  // in another pane is somewhere else entirely.
  let surface = RecordingSurface()
  let pane = view(surface: surface, clipboard: scratchClipboard("press"))

  pane.mouseDown(with: click(at: NSPoint(x: 10, y: 90)))

  #expect(surface.positions.count == 1)
  #expect(surface.buttons.count == 1)
}

// The pane's program is owed the mouse too, when it asked for it, and only its daemon can say
// whether it did - so every button and movement goes to the core as well as to the surface, and
// nothing is filtered here.

/// A pane on the seam, reporting its pointer the way a region wires it.
@MainActor
private func paneOnTheSeam(_ recording: RecordingSurface, _ recorder: RecordingDispatcher)
  -> SurfaceView
{
  let pane = view(surface: recording, clipboard: NSPasteboard.general, recorder: recorder)
  pane.onPointer = { Core.pointer(daemonID: "local", paneID: "p1w3r07bsd", $0) }
  return pane
}

private func mice(_ recorder: RecordingDispatcher) -> [Muster_Mouse] {
  recorder.requests.compactMap { if case .mouse(let mouse) = $0.payload { mouse } else { nil } }
}

@Test(.ownsTheSeam) @MainActor func aDragReachesThePanesDaemonMeasuredFromTheTop() {
  // The daemon compares the pointer against the terminal's size, which it knows in backing
  // pixels from the top left. Points from the bottom left would put a click on the mirror-image
  // row at half the distance.
  let recorder = RecordingDispatcher()
  let pane = paneOnTheSeam(RecordingSurface(), recorder)

  pane.mouseDown(with: click(at: NSPoint(x: 10, y: 90)))
  pane.mouseDragged(with: drag(to: NSPoint(x: 50, y: 40)))
  pane.mouseUp(with: release(at: NSPoint(x: 50, y: 40)))

  let sent = mice(recorder)
  #expect(sent.map(\.action) == ["press", "motion", "release"])
  #expect(sent.map(\.button) == ["left", "left", "left"])
  // No window, so the view falls back to the 2x it assumes everywhere else it needs a scale.
  #expect(sent.map(\.x) == [20, 100, 100])
  #expect(sent.map(\.y) == [20, 120, 120])
  #expect(sent.allSatisfy { $0.daemonID == "local" && $0.paneID == "p1w3r07bsd" })
}

@Test(.ownsTheSeam) @MainActor func shiftIsSentRatherThanDecidedHere() {
  // A shift-drag selects even over a program that asked for the mouse, and the daemon is what
  // applies that rule. A shell that dropped shifted events would decide it a second time.
  let recorder = RecordingDispatcher()
  let pane = paneOnTheSeam(RecordingSurface(), recorder)

  pane.mouseDown(with: mouse(.leftMouseDown, at: NSPoint(x: 10, y: 90), modifiers: [.shift]))

  #expect(mice(recorder).map(\.modifiers) == [["shift"]])
}

@Test(.ownsTheSeam) @MainActor func theRightAndMiddleButtonsCarryTheirNames() {
  let recorder = RecordingDispatcher()
  let recording = RecordingSurface()
  let pane = paneOnTheSeam(recording, recorder)

  pane.rightMouseDown(with: mouse(.rightMouseDown, at: NSPoint(x: 10, y: 90)))
  pane.rightMouseUp(with: mouse(.rightMouseUp, at: NSPoint(x: 10, y: 90)))
  pane.otherMouseDown(with: button(.otherMouseDown, .center))
  pane.otherMouseUp(with: button(.otherMouseUp, .center))

  #expect(mice(recorder).map(\.button) == ["right", "right", "middle", "middle"])
  #expect(mice(recorder).map(\.action) == ["press", "release", "press", "release"])
  #expect(recording.buttons.map(\.number) == [1, 1, 2, 2])
}

@Test(.ownsTheSeam) @MainActor func aButtonATerminalCannotReportReachesOnlyTheSurface() {
  // A terminal reports three buttons. Sent as "none", a back button would tell a program that
  // nothing was pressed, and its drag that nothing was held.
  let recorder = RecordingDispatcher()
  let recording = RecordingSurface()
  let pane = paneOnTheSeam(recording, recorder)

  pane.otherMouseDown(with: button(.otherMouseDown, CGMouseButton(rawValue: 3)!))
  pane.otherMouseDragged(with: button(.otherMouseDragged, CGMouseButton(rawValue: 3)!))

  #expect(mice(recorder).isEmpty)
  #expect(recording.buttons.map(\.number) == [3])
}

@Test(.ownsTheSeam) @MainActor func movingWithNothingHeldIsReportedToo() {
  // What a program asking for every movement - a TUI's hover - is owed. AppKit sends it only
  // to a view that tracks it.
  let recorder = RecordingDispatcher()
  let pane = paneOnTheSeam(RecordingSurface(), recorder)

  pane.mouseMoved(with: mouse(.mouseMoved, at: NSPoint(x: 10, y: 90)))

  #expect(pane.trackingAreas.contains { $0.options.contains(.mouseMoved) })
  #expect(mice(recorder).map(\.action) == ["motion"])
  #expect(mice(recorder).map(\.button) == ["none"])
}

@Test(.ownsTheSeam) @MainActor func aViewWithNoPaneTellsNoDaemonAboutTheMouse() {
  // The renderer check's shell is its surface's own, and no daemon holds it.
  let recorder = RecordingDispatcher()
  let recording = RecordingSurface()
  let pane = paneOnTheSeam(recording, recorder)
  pane.attach(typeable: false)

  pane.mouseDown(with: click(at: NSPoint(x: 10, y: 90)))
  pane.mouseMoved(with: mouse(.mouseMoved, at: NSPoint(x: 10, y: 90)))
  pane.rightMouseDown(with: mouse(.rightMouseDown, at: NSPoint(x: 10, y: 90)))

  #expect(mice(recorder).isEmpty)
  #expect(recording.buttons.map(\.number) == [0, 1])
}

@Test(.ownsTheSeam) @MainActor func copyPutsTheSelectionOnTheClipboard() {
  let clipboard = scratchClipboard("copy")
  let pane = view(surface: RecordingSurface(selection: "error: no such file"), clipboard: clipboard)

  pane.copy(nil)

  #expect(clipboard.string(forType: .string) == "error: no such file")
}

@Test(.ownsTheSeam) @MainActor func copyingNothingLeavesTheClipboardAlone() {
  // What every other terminal does, and what somebody who mistyped the chord expects. Clearing
  // it would lose whatever they copied a moment ago, from a keystroke that did nothing else.
  let clipboard = scratchClipboard("empty")
  clipboard.setString("kept", forType: .string)
  let pane = view(surface: RecordingSurface(selection: nil), clipboard: clipboard)

  pane.copy(nil)

  #expect(clipboard.string(forType: .string) == "kept")
}

@Test(.ownsTheSeam) @MainActor func theEditMenuGreysOutWhatWouldDoNothing() {
  // AppKit enables an item as soon as anything in the responder chain implements it, so
  // without this Copy looks available in a pane with nothing selected and then does nothing.
  let clipboard = scratchClipboard("validate")
  let copyItem = NSMenuItem(
    title: "Copy", action: #selector(SurfaceView.copy(_:)), keyEquivalent: "c")
  let pasteItem = NSMenuItem(
    title: "Paste", action: #selector(SurfaceView.paste(_:)), keyEquivalent: "v")

  let empty = view(surface: RecordingSurface(selection: nil), clipboard: clipboard)
  #expect(!empty.validateMenuItem(copyItem))
  #expect(!empty.validateMenuItem(pasteItem))

  clipboard.setString("something", forType: .string)
  let selected = view(surface: RecordingSurface(selection: "picked"), clipboard: clipboard)
  #expect(selected.validateMenuItem(copyItem))
  #expect(selected.validateMenuItem(pasteItem))
}

@Test(.ownsTheSeam) @MainActor func aPaneWhoseBridgeDiedStopsTakingKeystrokes() {
  // The dead square: libghostty paints its own "press any key to close the window" over a
  // surface whose command exited, and no key here will ever reach that - so a view that kept
  // sending them would put one refusal per keystroke into the log for a pane nobody can
  // reach. Reported once, and to the core, which is the only thing that can find out whether
  // the pane itself is gone.
  let recorder = RecordingDispatcher()
  let surface = RecordingSurface()
  let pane = view(surface: surface, clipboard: scratchClipboard("exited"), recorder: recorder)
  var reported: [Bool] = []
  pane.onProcessExited = { reported.append($0) }

  surface.onProcessExited?(false)
  pane.keyDown(with: key("h", keyCode: 0x04))

  #expect(reported == [false])
  let keys = recorder.requests.filter { if case .keyDown = $0.payload { true } else { false } }
  #expect(keys.isEmpty)
}

@Test(.ownsTheSeam) @MainActor func aBridgeThatDiesTwiceIsReportedOnce() {
  // libghostty may call this more than once for one surface, and a window that asked the
  // daemon to re-read its whole session per call would turn one dead pane into a round trip
  // per callback.
  let surface = RecordingSurface()
  let pane = view(surface: surface, clipboard: scratchClipboard("twice"))
  var reported = 0
  pane.onProcessExited = { _ in reported += 1 }

  surface.onProcessExited?(false)
  surface.onProcessExited?(false)

  #expect(reported == 1)
}

@Test(.ownsTheSeam) @MainActor func pasteSendsWhatIsOnTheClipboardAndNothingWhenItIsEmpty() {
  let recorder = RecordingDispatcher()
  let clipboard = scratchClipboard("paste")
  let pane = view(surface: RecordingSurface(), clipboard: clipboard, recorder: recorder)

  pane.paste(nil)
  clipboard.setString("cargo test", forType: .string)
  pane.paste(nil)

  // By payload rather than by count: an empty clipboard still writes a log record explaining
  // that nothing was sent, and that crosses the same seam.
  let pastes = recorder.requests.compactMap { request -> String? in
    guard case .paste(let paste) = request.payload else { return nil }
    return paste.text
  }
  #expect(pastes == ["cargo test"])
}

private func key(_ characters: String, keyCode: UInt16) -> NSEvent {
  NSEvent.keyEvent(
    with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
    context: nil, characters: characters, charactersIgnoringModifiers: characters,
    isARepeat: false, keyCode: keyCode)!
}

private func release(_ characters: String, keyCode: UInt16) -> NSEvent {
  NSEvent.keyEvent(
    with: .keyUp, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
    context: nil, characters: characters, charactersIgnoringModifiers: characters,
    isARepeat: false, keyCode: keyCode)!
}

/// The left command key changing, as AppKit reports it: the flags held after the change.
private func modifiers(_ flags: NSEvent.ModifierFlags) -> NSEvent {
  NSEvent.keyEvent(
    with: .flagsChanged, location: .zero, modifierFlags: flags, timestamp: 0, windowNumber: 0,
    context: nil, characters: "", charactersIgnoringModifiers: "", isARepeat: false,
    keyCode: 0x37)!
}

private func click(at point: NSPoint) -> NSEvent { mouse(.leftMouseDown, at: point) }
private func drag(to point: NSPoint) -> NSEvent { mouse(.leftMouseDragged, at: point) }
private func release(at point: NSPoint) -> NSEvent { mouse(.leftMouseUp, at: point) }

private func mouse(
  _ type: NSEvent.EventType, at point: NSPoint, modifiers: NSEvent.ModifierFlags = []
) -> NSEvent {
  // Window coordinates, which is what AppKit hands a view. With no window behind this one and
  // the view at the origin, the two spaces coincide - so what these assert on is the flip and
  // nothing else.
  NSEvent.mouseEvent(
    with: type, location: point, modifierFlags: modifiers, timestamp: 0, windowNumber: 0,
    context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
}

/// A button past the right one. AppKit's own constructor has no way to say which, so this goes
/// through CoreGraphics, which does.
private func button(_ type: CGEventType, _ button: CGMouseButton) -> NSEvent {
  NSEvent(
    cgEvent: CGEvent(
      mouseEventSource: nil, mouseType: type, mouseCursorPosition: .zero, mouseButton: button)!)!
}

// Sizing the text, which is a Muster action rather than a terminal setting - so it is
// rebindable, in the menu, and remembered across a launch like the sidebar it sits beside.

@Test(.ownsTheSeam) @MainActor func sizingTheTextReachesWhateverIsRenderingThePane() {
  let surface = RecordingSurface()
  let view = view(surface: surface, clipboard: scratchClipboard("fontsize"))

  view.setFontSizeOffset(3)
  view.setFontSizeOffset(0)

  #expect(surface.fontSizeOffsets == [3, 0])
}

@Test(.ownsTheSeam) @MainActor func aPaneWithNothingRenderingItYetIsNotAnError() {
  // The ordinary case at launch: the window applies the offset to every pane it holds, and a
  // pane whose bridge has not started has no surface to apply it to. Silently nothing, because
  // `attach` sizes it the moment one arrives.
  let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
  view.setFontSizeOffset(3)
}
