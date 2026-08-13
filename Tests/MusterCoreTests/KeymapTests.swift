import Testing

@testable import MusterCore

// The chords here are the ones people press without thinking, in a terminal that is not a
// text field. Getting one wrong is not a crash - it is a key that quietly does something
// close to right, which is the kind of bug that survives for months.

@Test("the macOS line-editing chords become their readline control codes")
func naturalTextEditing() {
  let keymap = Keymap()

  // ⌘⌫ is the one that prompted this: without it, backspace deletes one character and a
  // person holds it down instead.
  #expect(keymap.resolve(press(.backspace, .`super`)) == .text([0x15]))
  #expect(keymap.resolve(press(.arrowLeft, .`super`)) == .text([0x01]))
  #expect(keymap.resolve(press(.arrowRight, .`super`)) == .text([0x05]))
  #expect(keymap.resolve(press(.arrowLeft, .alt)) == .text([0x1b, UInt8(ascii: "b")]))
  #expect(keymap.resolve(press(.arrowRight, .alt)) == .text([0x1b, UInt8(ascii: "f")]))
}

@Test("an unmodified key is the pane's, not the keymap's")
func unmodifiedKeysPassThrough() {
  let keymap = Keymap()
  #expect(keymap.resolve(press(.backspace, [])) == .unbound)
  #expect(keymap.resolve(press(.arrowLeft, [])) == .unbound)
  #expect(keymap.resolve(press(.keyA, .`super`)) == .unbound)
}

@Test("which side of the keyboard a modifier came from does not change the chord")
func sideBitsAreIgnored() {
  // The encoder needs to know left command from right; a binding does not, and a keymap
  // that missed right-command would work on one half of the keyboard.
  let keymap = Keymap()
  #expect(keymap.resolve(press(.backspace, [.`super`, .superIsRight])) == .text([0x15]))
}

@Test("a key release never fires a binding")
func releasesDoNotFire() {
  // Under a kitty profile that reports releases, firing on both edges sends everything
  // twice - the same shape of bug that made typing `hello` produce `hheelllloo`.
  let keymap = Keymap()
  var event = press(.backspace, .`super`)
  event = KeyEvent(
    action: .release, key: event.key, modifiers: event.modifiers,
    consumedModifiers: event.consumedModifiers, text: event.text,
    unshiftedCodepoint: event.unshiftedCodepoint, isComposing: event.isComposing)
  #expect(keymap.resolve(event) == .unbound)
}

private func press(_ key: Key, _ modifiers: Modifiers) -> KeyEvent {
  KeyEvent(
    action: .press, key: key, modifiers: modifiers, consumedModifiers: [], text: "",
    unshiftedCodepoint: nil, isComposing: false)
}
