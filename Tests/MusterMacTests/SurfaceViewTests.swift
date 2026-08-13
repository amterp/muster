import AppKit
import MusterCore
import TestSupport
import Testing

@testable import MusterMac

// The bug that started all of this - every printable key reaching the pane twice - lived
// in this view, in an executable no test could import. The unit test on
// `CompositionArbiter` pins the decision; this pins the wiring, which is the other half:
// a decision made correctly and then called wrongly looks identical from the outside.
//
// No NSApplication, no window, no surface. A view and a keystroke.

@MainActor
private func view(_ recorder: SendRecorder) -> SurfaceView {
  let surface = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
  surface.attach(
    pane: PaneInput(
      channel: FakeChannel(name: "control", recorder: recorder), encoder: FakeEncoder()))
  return surface
}

@Test @MainActor func aTypedCharacterReachesThePaneExactlyOnce() {
  let recorder = SendRecorder()
  view(recorder).keyDown(with: key("h", keyCode: 0x04))

  // Once. The regression is two.
  #expect(recorder.intents == [.input([0x68])])
}

@Test @MainActor func typingAWordProducesThatWordAndNoMore() {
  // `hello` became `hheelllloo`, so spell it out: the failure was only visible in
  // aggregate, and a single-character case can pass while this one fails.
  let recorder = SendRecorder()
  let surface = view(recorder)
  for (character, code) in [("h", 0x04), ("e", 0x0e), ("l", 0x25), ("l", 0x25), ("o", 0x1f)] {
    surface.keyDown(with: key(character, keyCode: UInt16(code)))
  }

  let typed = recorder.intents.compactMap { intent -> [UInt8]? in
    if case .input(let bytes) = intent { return bytes }
    return nil
  }
  #expect(String(decoding: typed.flatMap { $0 }, as: UTF8.self) == "hello")
}

@Test @MainActor func aKeyThatEndsACompositionSendsWhatItProduced() {
  // The dead-key shape: a preedit is open, the next press resolves it, and what the pane
  // must receive is the composed character rather than the key that finished it.
  //
  // Only the commit half is reachable here. Keeping a composition open takes a real input
  // method - with none installed, AppKit's default handler commits the character
  // immediately - so "still composing sends nothing" is pinned on `CompositionArbiter`
  // instead, where it is a pure decision.
  let recorder = SendRecorder()
  let surface = view(recorder)
  surface.setMarkedText(
    "´", selectedRange: NSRange(location: 0, length: 1), replacementRange: NSRange())

  surface.keyDown(with: key("e", keyCode: 0x0e))

  // Once, as text - not the text and then the key as well.
  #expect(recorder.intents == [.input(Array("e".utf8))])
  #expect(!surface.hasMarkedText())
}

@Test @MainActor func committedTextFromOutsideAKeystrokeIsStillSent() {
  // A character picker or a service commits text with no key press behind it. Nothing
  // else is going to send that, so the view must.
  let recorder = SendRecorder()
  view(recorder).insertText("→", replacementRange: NSRange())

  #expect(recorder.intents == [.input(Array("→".utf8))])
}

@Test @MainActor func aWheelBecomesAScrollIntent() {
  let recorder = SendRecorder()
  let surface = view(recorder)
  guard let wheel = scroll(deltaY: 3) else { return }

  surface.scrollWheel(with: wheel)

  #expect(recorder.intents == [.scroll(direction: .up, lines: 3)])
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
