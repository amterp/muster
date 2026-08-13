import MusterCore
import MusterVT
import TestSupport
import Testing

/// What Muster puts on a pane's input for a given keystroke.
///
/// The encoder is libghostty-vt's, so these do not test escape-sequence generation -
/// upstream does that. What they pin is everything around it that is ours: that the
/// modifier bits line up with the ABI, that a mode profile reaches the encoder, that a
/// composing keystroke stays out of the pane, and what the conservative default actually
/// costs. That last one is a kill-criterion question, and it is answered here in bytes
/// rather than in prose.

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

@Test("shift+enter survives the conservative default")
func shiftEnterSurvivesTheDefault() throws {
  // The distinction an agent needs most, and the one this plan assumed would be the
  // first casualty of guessing low. It is not: ghostty encodes shift+enter as a
  // fixterms sequence in every mode, not only under kitty
  // (`src/input/function_keys.zig:199`, an entry with no mode qualifier). So an agent
  // that treats shift+enter as newline and enter as submit can still tell them apart
  // through a pane Muster knows nothing about.
  let legacy = try KeyEncoder(profile: .unknownPane)

  #expect(try legacy.encode(KeyEvent(key: .enter)) == Array("\r".utf8))
  #expect(
    try legacy.encode(KeyEvent(key: .enter, modifiers: .shift)) == Array("\u{1b}[27;2;13~".utf8))
}

@Test("application cursor mode changes what the arrow keys send")
func applicationCursorKeysChangeArrows() throws {
  // The one guess that breaks a program rather than degrading it: vim and less put the
  // cursor keys in application mode, and a client that guesses wrong sends a sequence
  // the program does not recognize.
  let normal = try KeyEncoder(profile: .unknownPane)
  #expect(try normal.encode(KeyEvent(key: .arrowUp)) == Array("\u{1b}[A".utf8))

  let application = try KeyEncoder(profile: TerminalModeProfile(applicationCursorKeys: true))
  #expect(try application.encode(KeyEvent(key: .arrowUp)) == Array("\u{1b}OA".utf8))
}

@Test("a composing keystroke never reaches the pane")
func composingKeystrokesAreWithheld() throws {
  let encoder = try KeyEncoder(profile: .unknownPane)

  // Mid-composition, the keystroke belongs to the input method. Sending it too would
  // deliver the romaji alongside whatever it composes into.
  let composing = KeyEvent(key: .keyA, text: "a", unshiftedCodepoint: "a", isComposing: true)
  #expect(try encoder.encode(composing).isEmpty)
}

@Test("modifier bits agree with the ABI they are cast to")
func modifierBitsMatchTheABI() throws {
  // Modifiers is declared to match GhosttyMods so the seam is a cast. If a pin bump
  // renumbers those bits, every chord silently encodes as a different one - so the
  // check is that a known chord still produces its known bytes.
  let encoder = try KeyEncoder(profile: .unknownPane)

  #expect(try encoder.encode(KeyEvent(key: .keyC, modifiers: .control)) == [0x03])
  #expect(try encoder.encode(KeyEvent(key: .keyA, modifiers: .control)) == [0x01])
}

@Test("the option key composes text unless it is configured to act as alt")
func optionActsAsAltChangesWhatAltProduces() throws {
  let event = KeyEvent(key: .keyB, modifiers: .alt, text: "b", unshiftedCodepoint: "b")

  // The macOS default, and the reason it is not simply "alt sends escape": option is a
  // composing key on this platform, so a Mac user pressing option+e expects an accent
  // rather than a meta chord. With it off, alt is dropped and the text stands alone.
  let composing = try KeyEncoder(profile: .unknownPane)
  #expect(try composing.encode(event) == Array("b".utf8))

  let asAlt = try KeyEncoder(profile: TerminalModeProfile(optionActsAsAlt: .always))
  #expect(try asAlt.encode(event) == Array("\u{1b}b".utf8))
}
