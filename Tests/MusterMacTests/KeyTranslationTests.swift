import AppKit
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
  let translated = event.musterKeyEvent(action: "press", isComposing: false)

  #expect(translated.key == "KeyA")
  #expect(translated.text == "a")
  #expect(translated.modifiers.isEmpty)
  #expect(translated.action == "press")
}

@Test("shift is reported as held and as spent on the character")
func shiftIsConsumed() throws {
  // macOS never says which modifiers a layout used to produce a character, so ghostty
  // assumes everything except control and command contributed. Shift did here, and the
  // encoder needs to know that or it will report the shift twice.
  let event = try #require(key("A", unshifted: "a", keyCode: 0x00, flags: .shift))
  let translated = event.musterKeyEvent(action: "press", isComposing: false)

  #expect(translated.modifiers.contains("shift"))
  #expect(translated.consumedModifiers.contains("shift"))
  #expect(translated.text == "A")
}

@Test("control and command never count as spent on the character")
func controlAndCommandAreNotConsumed() throws {
  let control = try #require(key("\u{03}", unshifted: "c", keyCode: 0x08, flags: .control))
  let translatedControl = control.musterKeyEvent(action: "press", isComposing: false)
  #expect(translatedControl.modifiers.contains("control"))
  #expect(!translatedControl.consumedModifiers.contains("control"))

  let command = try #require(key("v", keyCode: 0x09, flags: .command))
  let translatedCommand = command.musterKeyEvent(action: "press", isComposing: false)
  #expect(translatedCommand.modifiers.contains("super"))
  #expect(!translatedCommand.consumedModifiers.contains("super"))
}

@Test("a control character is undone, because the encoder applies control itself")
func controlCharacterIsUnwound() throws {
  // AppKit hands back 0x03 for ctrl+C. Passing that on as text would have the encoder
  // apply control to an already-controlled character.
  let event = try #require(key("\u{03}", unshifted: "c", keyCode: 0x08, flags: .control))
  let translated = event.musterKeyEvent(action: "press", isComposing: false)

  #expect(translated.text == "c")
  #expect(translated.key == "KeyC")
}

@Test("a function key is a key, not a glyph nobody has")
func functionKeysCarryNoText() throws {
  // Arrows and function keys arrive as private-use codepoints. Sent as text they would
  // type garbage into the pane.
  let event = try #require(key("\u{F700}", keyCode: 0x7e))
  let translated = event.musterKeyEvent(action: "press", isComposing: false)

  #expect(translated.key == "ArrowUp")
  #expect(translated.text == "")
}

@Test("a key macOS names but libghostty does not still types its character")
func unmappableKeysDegradeToText() throws {
  // The JIS kana and eisu keys are the only two at this pin. Dropping the keystroke would
  // be worse than reporting it unidentified with its text intact.
  let event = try #require(key("z", keyCode: 0x68))
  let translated = event.musterKeyEvent(action: "press", isComposing: false)

  #expect(translated.key == "unidentified")
  #expect(translated.text == "z")
}

@Test("a repeat is reported as a repeat, not as a fresh press")
func repeatsAreDistinct() throws {
  let event = try #require(key("a", keyCode: 0x00, isARepeat: true))
  let translated = event.musterKeyEvent(action: "repeated", isComposing: false)
  #expect(translated.action == "repeated")
}

@Test("an option chord carries both readings, because only the layout knows the second")
func optionChordsCarryTheirOtherReading() throws {
  // Option is two keys on macOS: a modifier, and one that composes characters. Which it is
  // for a given person is configuration, so the shell reports what the layout did and what
  // it would have done, and the core picks. Deriving the second from the first is not
  // possible - nothing about `†` says it came from `t`.
  let event = try #require(key("†", unshifted: "t", keyCode: 0x11, flags: .option))
  let translated = event.musterKeyEvent(action: "press", isComposing: false)

  #expect(translated.text == "†")
  #expect(translated.textWithoutOption == "t")
  #expect(translated.modifiers.contains("alt"))
  #expect(translated.consumedModifiers.contains("alt"))
}

@Test("a keystroke with no option held reports no second reading")
func plainKeystrokesCarryOneReading() throws {
  // The two only differ while option is down, and translating twice on every keypress
  // would cost the input path a layout lookup nothing reads.
  let event = try #require(key("a", keyCode: 0x00))
  let translated = event.musterKeyEvent(action: "press", isComposing: false)
  #expect(translated.textWithoutOption.isEmpty)
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
