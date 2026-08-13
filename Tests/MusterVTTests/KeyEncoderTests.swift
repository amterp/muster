import MusterCore
import MusterVT
import TestSupport
import Testing

/// The survey: what Muster puts on a pane's input for the keys people press constantly.
///
/// One matrix with one reason rather than nineteen behaviors, and its oracle is upstream,
/// so it stays a rendered snapshot instead of becoming conformance cases with nineteen
/// manufactured justifications. A port reproduces the same rendering from the same list.
///
/// Everything this file used to assert about *our* decisions - that the modifier bits line
/// up with the ABI, that a mode profile reaches the encoder, that a composing keystroke
/// stays out of the pane - is now corpus/conformance/key-encoder.json, run by
/// KeyEncoderConformanceTests.

/// The matrix, as one readable file. Every row is a keystroke a user makes constantly.
private let commonKeystrokes: [(name: String, event: KeyEvent)] = [
  ("a", KeyEvent(key: .keyA, text: "a", unshiftedCodepoint: "a")),
  ("shift+a", KeyEvent(key: .keyA, modifiers: .shift, text: "A", unshiftedCodepoint: "a")),
  ("ctrl+c", KeyEvent(key: .keyC, modifiers: .control, unshiftedCodepoint: "c")),
  ("enter", KeyEvent(key: .enter)),
  ("shift+enter", KeyEvent(key: .enter, modifiers: .shift)),
  ("tab", KeyEvent(key: .tab)),
  ("shift+tab", KeyEvent(key: .tab, modifiers: .shift)),
  ("escape", KeyEvent(key: .escape)),
  ("backspace", KeyEvent(key: .backspace)),
  ("arrow up", KeyEvent(key: .arrowUp)),
  ("arrow down", KeyEvent(key: .arrowDown)),
  ("home", KeyEvent(key: .home)),
  ("end", KeyEvent(key: .end)),
  ("page up", KeyEvent(key: .pageUp)),
  ("delete", KeyEvent(key: .delete)),
  ("f1", KeyEvent(key: .f1)),
  ("f12", KeyEvent(key: .f12)),
  ("alt+b", KeyEvent(key: .keyB, modifiers: .alt, text: "b", unshiftedCodepoint: "b")),
  ("ctrl+alt+delete", KeyEvent(key: .delete, modifiers: [.control, .alt])),
]

private func renderMatrix(profile: TerminalModeProfile) throws -> String {
  let encoder = try KeyEncoder(profile: profile)
  let width = commonKeystrokes.map(\.name.count).max() ?? 0
  return
    try commonKeystrokes
    .map { name, event in
      let bytes = try encoder.encode(event)
      let padding = String(repeating: " ", count: width - name.count)
      return "\(name)\(padding)  \(readable(bytes))"
    }
    .joined(separator: "\n") + "\n"
}

/// Bytes as a reviewer reads them: ESC for 0x1b, ^C for control characters, printable
/// characters as themselves. A hex dump would be exact and unreadable, and nobody would
/// notice the day it changed.
private func readable(_ bytes: [UInt8]) -> String {
  guard !bytes.isEmpty else { return "(nothing)" }
  return
    bytes
    .map { byte in
      switch byte {
      case 0x1b: "ESC"
      case 0x7f: "DEL"
      case 0x00...0x1f: "^\(Character(UnicodeScalar(byte + 0x40)))"
      case 0x20: "SP"
      default: String(UnicodeScalar(byte))
      }
    }
    .joined(separator: " ")
}

@Test("what an unknown pane gets for the keys people press constantly")
func legacyEncodingMatrix() throws {
  try Snapshot.expect(
    renderMatrix(profile: .unknownPane), named: "keys-unknown-pane.txt")
}

@Test("what the same keys become once a pane's kitty flags are known")
func kittyEncodingMatrix() throws {
  // Not reachable today - it needs mode state herdr does not expose - and recorded
  // anyway, because the difference between these two files is exactly what the upstream
  // ask is worth.
  try Snapshot.expect(
    renderMatrix(profile: .herdrTUI), named: "keys-herdr-tui.txt")
}
