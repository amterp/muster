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
  Core.dispatcher = recorder
  let surface = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
  surface.attach(typeable: true)
  return surface
}

@Test @MainActor func aTypedCharacterCrossesTheSeamExactlyOnce() {
  let recorder = RecordingDispatcher()
  view(recorder).keyDown(with: key("h", keyCode: 0x04))

  // Once. The regression is two.
  #expect(recorder.requests.count == 1)
  #expect(recorder.requests.first?.keyDown.key.key == "KeyH")
  #expect(recorder.requests.first?.keyDown.key.text == "h")
  #expect(recorder.requests.first?.keyDown.key.action == "press")
}

@Test @MainActor func typingAWordSendsThatWordAndNoMore() {
  // `hello` became `hheelllloo`, so spell it out: the failure was only visible in
  // aggregate, and a single-character case can pass while this one fails.
  let recorder = RecordingDispatcher()
  let surface = view(recorder)
  for (character, code) in [("h", 0x04), ("e", 0x0e), ("l", 0x25), ("l", 0x25), ("o", 0x1f)] {
    surface.keyDown(with: key(character, keyCode: UInt16(code)))
  }

  #expect(recorder.requests.map { $0.keyDown.key.text }.joined() == "hello")
}

@Test @MainActor func aKeyPressCarriesTheCompositionSignalsRatherThanResolvingThem() {
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

@Test @MainActor func committedTextFromOutsideAKeystrokeIsStillSent() {
  // A character picker or a service commits text with no key press behind it. Nothing else
  // is going to send that, so the view must.
  let recorder = RecordingDispatcher()
  view(recorder).insertText("→", replacementRange: NSRange())

  #expect(recorder.requests.map { $0.sendText.text } == ["→"])
}

@Test @MainActor func aWheelIsReportedRatherThanSent() {
  // The device's own delta, unscaled and unrounded. How many lines that is worth depends on
  // `scroll_multiplier`, so the core decides it - a shell that turned a delta into lines here
  // would be a second place that answer lives, and the two would drift.
  //
  // Reported rather than sent, because a wheel is addressed to a pane and this view does not
  // know which one it is showing. Which pane it names is pinned a layer up, where the id is.
  let surface = view(recorder())
  var asked: [(String, Double)] = []
  surface.onScroll = { asked.append(($0, $1)) }
  guard let event = wheel(deltaY: 3) else { return }

  surface.scrollWheel(with: event)

  #expect(asked.map(\.0) == ["up"])
  #expect(asked.map(\.1) == [3])
}

@Test @MainActor func aCellIsReportedInPointsRatherThanBackingPixels() {
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

@Test @MainActor func aSurfaceNothingHasSizedYetReportsNoCellRatherThanZero() {
  // Zero would reach the core as a cell of no width and be divided by. Nil says "could not
  // measure", which the core answers with the daemon's own step.
  let surface = view(surface: RecordingSurface(), clipboard: NSPasteboard.general)

  #expect(surface.cellPointSize == nil)
}

@Test @MainActor func aWheelOverAPaneNeverAsksForTheKeyboard() {
  // The whole point of the feature: reading one agent's output while typing into another. A
  // scroll that also focused would make that impossible in exactly the case it exists for.
  let surface = view(recorder())
  var focused = false
  surface.onClick = { focused = true }
  guard let event = wheel(deltaY: 3) else { return }

  surface.scrollWheel(with: event)

  #expect(focused == false)
}

@Test @MainActor func aViewWithNoPaneSendsNothingRatherThanRefusalsPerKeystroke() {
  // A bare `muster` is the renderer check, and every key it swallows is expected. Sending
  // them anyway would fill the log with a refusal per keystroke for a state that is normal.
  let recorder = RecordingDispatcher()
  Core.dispatcher = recorder
  let surface = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
  surface.attach(typeable: false)

  surface.keyDown(with: key("h", keyCode: 0x04))

  #expect(recorder.requests.isEmpty)
}

// Selection and the clipboard, which is the one input path that never reaches the core: the
// grid libghostty painted is where a drag lands, so the oracle here is the surface rather than
// the seam.

/// Records what the view asked of the thing rendering it, and answers with a fixed selection.
@MainActor
private final class RecordingSurface: PaneSurface {
  var positions: [NSPoint] = []
  var buttons: [Bool] = []
  var selectedText: String?
  var onProcessExited: (@MainActor (Bool) -> Void)?
  /// Every offset asked for, in order, so a test can tell "sized once" from "sized twice".
  var fontSizeOffsets: [Int32] = []
  /// In backing pixels, as libghostty answers. Nil is a surface nothing has sized yet.
  var cellPixelSize: (width: UInt32, height: UInt32)?

  init(selection: String? = nil) { selectedText = selection }

  func setSize(width: UInt32, height: UInt32) {}
  func setFocus(_ focused: Bool) {}
  func setFontSizeOffset(_ points: Int32) -> [String] {
    fontSizeOffsets.append(points)
    return []
  }
  func mouseMoved(to point: NSPoint, modifiers: NSEvent.ModifierFlags) { positions.append(point) }
  func leftMouse(pressed: Bool, modifiers: NSEvent.ModifierFlags) { buttons.append(pressed) }
}

@MainActor
private func view(
  surface: RecordingSurface, clipboard: NSPasteboard,
  recorder: RecordingDispatcher = RecordingDispatcher()
) -> SurfaceView {
  Core.dispatcher = recorder
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

@Test @MainActor func aDragArrivesInTheSurfacesOwnCoordinates() {
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
  #expect(surface.buttons == [true, false])
}

@Test @MainActor func aPressCarriesThePointerBeforeTheButton() {
  // libghostty holds the pointer position separately from the button, so a press reported
  // without one starts the selection wherever the pointer was last seen - which after a click
  // in another pane is somewhere else entirely.
  let surface = RecordingSurface()
  let pane = view(surface: surface, clipboard: scratchClipboard("press"))

  pane.mouseDown(with: click(at: NSPoint(x: 10, y: 90)))

  #expect(surface.positions.count == 1)
  #expect(surface.buttons.count == 1)
}

@Test @MainActor func copyPutsTheSelectionOnTheClipboard() {
  let clipboard = scratchClipboard("copy")
  let pane = view(surface: RecordingSurface(selection: "error: no such file"), clipboard: clipboard)

  pane.copy(nil)

  #expect(clipboard.string(forType: .string) == "error: no such file")
}

@Test @MainActor func copyingNothingLeavesTheClipboardAlone() {
  // What every other terminal does, and what somebody who mistyped the chord expects. Clearing
  // it would lose whatever they copied a moment ago, from a keystroke that did nothing else.
  let clipboard = scratchClipboard("empty")
  clipboard.setString("kept", forType: .string)
  let pane = view(surface: RecordingSurface(selection: nil), clipboard: clipboard)

  pane.copy(nil)

  #expect(clipboard.string(forType: .string) == "kept")
}

@Test @MainActor func theEditMenuGreysOutWhatWouldDoNothing() {
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

@Test @MainActor func aPaneWhoseBridgeDiedStopsTakingKeystrokes() {
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

@Test @MainActor func aBridgeThatDiesTwiceIsReportedOnce() {
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

@Test @MainActor func pasteSendsWhatIsOnTheClipboardAndNothingWhenItIsEmpty() {
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

private func click(at point: NSPoint) -> NSEvent { mouse(.leftMouseDown, at: point) }
private func drag(to point: NSPoint) -> NSEvent { mouse(.leftMouseDragged, at: point) }
private func release(at point: NSPoint) -> NSEvent { mouse(.leftMouseUp, at: point) }

private func mouse(_ type: NSEvent.EventType, at point: NSPoint) -> NSEvent {
  // Window coordinates, which is what AppKit hands a view. With no window behind this one and
  // the view at the origin, the two spaces coincide - so what these assert on is the flip and
  // nothing else.
  NSEvent.mouseEvent(
    with: type, location: point, modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil,
    eventNumber: 0, clickCount: 1, pressure: 1)!
}

// Sizing the text, which is a Muster action rather than a terminal setting - so it is
// rebindable, in the menu, and remembered across a launch like the sidebar it sits beside.

@Test @MainActor func sizingTheTextReachesWhateverIsRenderingThePane() {
  let surface = RecordingSurface()
  let view = view(surface: surface, clipboard: scratchClipboard("fontsize"))

  view.setFontSizeOffset(3)
  view.setFontSizeOffset(0)

  #expect(surface.fontSizeOffsets == [3, 0])
}

@Test @MainActor func aPaneWithNothingRenderingItYetIsNotAnError() {
  // The ordinary case at launch: the window applies the offset to every pane it holds, and a
  // pane whose bridge has not started has no surface to apply it to. Silently nothing, because
  // `attach` sizes it the moment one arrives.
  let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
  view.setFontSizeOffset(3)
}
