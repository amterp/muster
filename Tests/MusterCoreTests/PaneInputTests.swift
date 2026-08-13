import TestSupport
import Testing

@testable import MusterCore

// A driver over the first sequential concept in the corpus: a case is a list of steps and
// the expectation is the ordered trace of everything that went out, across every channel.
// Cases and their reasoning live in corpus/conformance/pane-input.json.

@Test("pane input routing")
func paneInputConformance() throws {
  let corpus = try Conformance.load("pane-input.json")

  let ran = corpus.run { given in
    let recorder = SendRecorder()
    let pane = PaneInput(
      channel: FakeChannel(name: "control", recorder: recorder),
      serverChannel: serverChannel(from: given["daemon"], recorder: recorder),
      encoder: FakeEncoder())

    for step in given["steps"]?.arrayValue ?? [] {
      try apply(step, to: pane)
    }

    return .fields([
      "trace": .array(recorder.sends.map { describe(channel: $0.channel, intent: $0.intent) })
    ])
  }

  #expect(ran == corpus.cases.count)
  #expect(ran > 0)
}

/// The daemon channel a case asks for, or none.
///
/// Absent means no daemon at all - the degraded arrangement where the app has to guess.
/// `refuses` means reachable but declining, which is the wedged-daemon state and a
/// different path from absence.
private func serverChannel(from value: JSONValue?, recorder: SendRecorder) -> FakeChannel? {
  guard let value else { return nil }
  let refuses = value["refuses"]?.boolValue ?? false
  return FakeChannel(
    name: "daemon", recorder: recorder, encodesServerSide: true, accepts: { _ in !refuses })
}

private func apply(_ step: JSONValue, to pane: PaneInput) throws {
  if let send = step["send"] {
    guard let name = send["key"]?.stringValue, let key = Key(rawValue: name) else {
      throw CaseError("`send.key` is missing or not a W3C key name")
    }
    guard let modifiers = Modifiers(names: send.strings("modifiers")) else {
      throw CaseError("`send.modifiers` names something that is not a modifier")
    }
    pane.send(
      KeyEvent(
        action: .press, key: key, modifiers: modifiers, consumedModifiers: [],
        text: send["text"]?.stringValue ?? "", unshiftedCodepoint: nil, isComposing: false))
    return
  }
  if let text = step["paste"]?.stringValue {
    pane.paste(text: text)
    return
  }
  if let scroll = step["scroll"] {
    guard let name = scroll["direction"]?.stringValue,
      let direction = PaneIntent.ScrollDirection(rawValue: name),
      let lines = scroll["lines"]?.intValue
    else {
      throw CaseError("`scroll` needs a known `direction` and `lines`")
    }
    pane.scroll(direction: direction, lines: UInt16(lines))
    return
  }
  throw CaseError("a step must be one of send, paste, scroll")
}

private func describe(channel: String, intent: PaneIntent) -> JSONValue {
  switch intent {
  case .input(let bytes):
    .fields([
      "channel": .string(channel), "intent": "input",
      "bytes_hex": .string(bytes.map { String(format: "%02x", $0) }.joined()),
    ])
  case .text(let text):
    .fields(["channel": .string(channel), "intent": "text", "text": .string(text)])
  case .key(let name):
    .fields(["channel": .string(channel), "intent": "key", "name": .string(name)])
  case .scroll(let direction, let lines):
    .fields([
      "channel": .string(channel), "intent": "scroll",
      "direction": .string(direction.rawValue), "lines": .number(Double(lines)),
    ])
  }
}

private struct CaseError: Error, CustomStringConvertible {
  let description: String
  init(_ description: String) { self.description = description }
}
