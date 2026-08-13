import MusterCore
import MusterVT
import TestSupport
import Testing

// A driver. The cases live in corpus/conformance/key-encoder.json.

@Test("key encoding")
func keyEncoderConformance() throws {
  let corpus = try Conformance.load("key-encoder.json")

  let ran = corpus.run { given in
    let encoder = try KeyEncoder(profile: try profile(from: given["profile"]))
    let bytes = try encoder.encode(try keyEvent(from: given))
    return .fields(["bytes_hex": .string(bytes.map { String(format: "%02x", $0) }.joined())])
  }

  #expect(ran == corpus.cases.count)
  #expect(ran > 0)
}

/// A named preset, or the fields that differ from the conservative default.
private func profile(from value: JSONValue?) throws -> TerminalModeProfile {
  switch value?.stringValue {
  case "unknownPane": return .unknownPane
  case "herdrTUI": return .herdrTUI
  case let name?: throw CaseError("unknown profile preset \(name)")
  case nil: break
  }
  guard let value, case .object = value else {
    throw CaseError("`profile` must be a preset name or an object of settings")
  }
  let optionAsAlt = value["optionActsAsAlt"]?.stringValue
  guard let option = optionAsAlt.map({ TerminalModeProfile.OptionAsAlt(rawValue: $0) }) ?? .never
  else {
    throw CaseError("`optionActsAsAlt` is \(optionAsAlt ?? "nil"), not a known setting")
  }
  return TerminalModeProfile(
    kittyFlags: UInt8(value["kittyFlags"]?.intValue ?? 0),
    applicationCursorKeys: value["applicationCursorKeys"]?.boolValue ?? false,
    applicationKeypad: value["applicationKeypad"]?.boolValue ?? false,
    altSendsEscapePrefix: value["altSendsEscapePrefix"]?.boolValue ?? true,
    modifyOtherKeys: value["modifyOtherKeys"]?.boolValue ?? false,
    bracketedPaste: value["bracketedPaste"]?.boolValue ?? false,
    optionActsAsAlt: option)
}

private func keyEvent(from given: JSONValue) throws -> KeyEvent {
  guard let name = given["key"]?.stringValue, let key = Key(rawValue: name) else {
    throw CaseError("`key` is missing or not a W3C key name")
  }
  guard let modifiers = Modifiers(names: given.strings("modifiers")) else {
    throw CaseError("`modifiers` names something that is not a modifier")
  }
  return KeyEvent(
    key: key, modifiers: modifiers, text: given["text"]?.stringValue ?? "",
    unshiftedCodepoint: given["unshiftedCodepoint"]?.stringValue?.unicodeScalars.first,
    isComposing: given["isComposing"]?.boolValue ?? false)
}

private struct CaseError: Error, CustomStringConvertible {
  let description: String
  init(_ description: String) { self.description = description }
}
