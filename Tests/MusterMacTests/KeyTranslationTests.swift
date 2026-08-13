import AppKit
import MusterCore
import Testing

@testable import MusterMac

// These heuristics are ported from ghostty and have years of keyboard layouts behind them,
// which is exactly why they were never tested here: they looked settled. They were also
// unreachable, living in an executable target no test target may import. Both of those are
// now false.
//
// `NSEvent.keyEvent(with:)` builds a real event with no NSApplication, window or run loop
// involved, so the whole translation is assertable offline.

@Test("a plain letter arrives as its key and its text")
func plainLetter() throws {
  let event = try #require(key("a", keyCode: 0x00))
  let translated = try #require(event.musterKeyEvent(action: .press, isComposing: false))

  #expect(translated.key == .keyA)
  #expect(translated.text == "a")
  #expect(translated.modifiers == [])
  #expect(translated.action == .press)
}

@Test("shift is reported as held and as spent on the character")
func shiftIsConsumed() throws {
  // macOS never says which modifiers a layout used to produce a character, so ghostty
  // assumes everything except control and command contributed. Shift did here, and the
  // encoder needs to know that or it will report the shift twice.
  let event = try #require(key("A", unshifted: "a", keyCode: 0x00, flags: .shift))
  let translated = try #require(event.musterKeyEvent(action: .press, isComposing: false))

  #expect(translated.modifiers.contains(.shift))
  #expect(translated.consumedModifiers.contains(.shift))
  #expect(translated.text == "A")
}

@Test("control and command never count as spent on the character")
func controlAndCommandAreNotConsumed() throws {
  let control = try #require(key("\u{03}", unshifted: "c", keyCode: 0x08, flags: .control))
  let translatedControl = try #require(control.musterKeyEvent(action: .press, isComposing: false))
  #expect(translatedControl.modifiers.contains(.control))
  #expect(!translatedControl.consumedModifiers.contains(.control))

  let command = try #require(key("v", keyCode: 0x09, flags: .command))
  let translatedCommand = try #require(command.musterKeyEvent(action: .press, isComposing: false))
  #expect(translatedCommand.modifiers.contains(.`super`))
  #expect(!translatedCommand.consumedModifiers.contains(.`super`))
}

@Test("a control character is undone, because the encoder applies control itself")
func controlCharacterIsUnwound() throws {
  // AppKit hands back 0x03 for ctrl+C. Passing that on as text would have the encoder
  // apply control to an already-controlled character.
  let event = try #require(key("\u{03}", unshifted: "c", keyCode: 0x08, flags: .control))
  let translated = try #require(event.musterKeyEvent(action: .press, isComposing: false))

  #expect(translated.text == "c")
  #expect(translated.key == .keyC)
}

@Test("a function key is a key, not a glyph nobody has")
func functionKeysCarryNoText() throws {
  // Arrows and function keys arrive as private-use codepoints. Sent as text they would
  // type garbage into the pane.
  let event = try #require(key("\u{F700}", keyCode: 0x7e))
  let translated = try #require(event.musterKeyEvent(action: .press, isComposing: false))

  #expect(translated.key == .arrowUp)
  #expect(translated.text == "")
}

@Test("a key macOS names but libghostty does not still types its character")
func unmappableKeysDegradeToText() throws {
  // The JIS kana and eisu keys are the only two at this pin. Dropping the keystroke would
  // be worse than reporting it unidentified with its text intact.
  let event = try #require(key("z", keyCode: 0x68))
  let translated = try #require(event.musterKeyEvent(action: .press, isComposing: false))

  #expect(translated.key == .unidentified)
  #expect(translated.text == "z")
}

@Test("a repeat is reported as a repeat, not as a fresh press")
func repeatsAreDistinct() throws {
  let event = try #require(key("a", keyCode: 0x00, isARepeat: true))
  let translated = try #require(event.musterKeyEvent(action: .repeated, isComposing: false))
  #expect(translated.action == .repeated)
}

/// Builds the keystroke AppKit would have delivered.
private func key(
  _ characters: String,
  unshifted: String? = nil,
  keyCode: UInt16,
  flags: NSEvent.ModifierFlags = [],
  isARepeat: Bool = false
) -> NSEvent? {
  NSEvent.keyEvent(
    with: .keyDown,
    location: .zero,
    modifierFlags: flags,
    timestamp: 0,
    windowNumber: 0,
    context: nil,
    characters: characters,
    charactersIgnoringModifiers: unshifted ?? characters,
    isARepeat: isARepeat,
    keyCode: keyCode)
}
