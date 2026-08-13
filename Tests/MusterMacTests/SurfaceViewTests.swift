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

/// Records what the shell asked of the core, and answers as the core would.
private final class RecordingDispatcher: Dispatcher, @unchecked Sendable {
  private(set) var requests: [Muster_Request] = []

  func dispatch(_ request: [UInt8]) -> [UInt8] {
    if let decoded = try? Muster_Request(serializedBytes: request) {
      requests.append(decoded)
    }
    var response = Muster_Response()
    response.ok = Muster_Ok()
    return (try? response.serializedBytes()) ?? []
  }
}

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

@Test @MainActor func aWheelBecomesAScrollIntent() {
  let recorder = RecordingDispatcher()
  let surface = view(recorder)
  guard let wheel = scroll(deltaY: 3) else { return }

  surface.scrollWheel(with: wheel)

  #expect(recorder.requests.count == 1)
  #expect(recorder.requests[0].scroll.direction == "up")
  #expect(recorder.requests[0].scroll.lines == 3)
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

private func key(_ characters: String, keyCode: UInt16) -> NSEvent {
  NSEvent.keyEvent(
    with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
    context: nil, characters: characters, charactersIgnoringModifiers: characters,
    isARepeat: false, keyCode: keyCode)!
}

private func scroll(deltaY: CGFloat) -> NSEvent? {
  // Wheel events have no public constructor, so this goes through CGEvent, which does.
  guard
    let event = CGEvent(
      scrollWheelEvent2Source: nil, units: .line, wheelCount: 1, wheel1: 0, wheel2: 0, wheel3: 0)
  else { return nil }
  event.setDoubleValueField(.scrollWheelEventPointDeltaAxis1, value: Double(deltaY))
  event.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1, value: Double(deltaY))
  return NSEvent(cgEvent: event)
}
