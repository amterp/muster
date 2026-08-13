import TestSupport
import Testing

@testable import MusterCore

// A driver. The cases live in corpus/conformance/keymap.json - see that file for what each
// chord protects and why Muster binds it at all.

@Test("keymap resolution")
func keymapConformance() throws {
  let corpus = try Conformance.load("keymap.json")
  let keymap = Keymap()

  let ran = corpus.run { given in
    let event = try keyEvent(from: given)

    return switch keymap.resolve(event) {
    case .unbound: .fields(["kind": "unbound"])
    case .text(let bytes): .fields(["kind": "text", "bytes_hex": .string(hex(bytes))])
    case .serverEncoded(let name): .fields(["kind": "serverEncoded", "key": .string(name)])
    case .action: .fields(["kind": "action"])
    }
  }

  #expect(ran == corpus.cases.count)
  #expect(ran > 0)
}

/// Reads the corpus's spelling of a keystroke.
///
/// Strict about names it does not recognize. A case file saying `"comand"` should fail
/// loudly, not quietly test an unmodified key and pass.
private func keyEvent(from given: JSONValue) throws -> KeyEvent {
  guard let name = given["key"]?.stringValue, let key = Key(rawValue: name) else {
    throw CaseError("`key` is missing or not a W3C key name")
  }
  guard let modifiers = Modifiers(names: given.strings("modifiers")) else {
    throw CaseError("`modifiers` names something that is not a modifier")
  }
  let action: KeyEvent.Action =
    switch given["action"]?.stringValue {
    case nil, "press": .press
    case "release": .release
    case let other: throw CaseError("`action` is \(other ?? "nil"), not press or release")
    }

  return KeyEvent(
    action: action, key: key, modifiers: modifiers, consumedModifiers: [],
    text: given["text"]?.stringValue ?? "", unshiftedCodepoint: nil, isComposing: false)
}

private struct CaseError: Error, CustomStringConvertible {
  let description: String
  init(_ description: String) { self.description = description }
}

private func hex(_ bytes: [UInt8]) -> String {
  bytes.map { String(format: "%02x", $0) }.joined()
}
