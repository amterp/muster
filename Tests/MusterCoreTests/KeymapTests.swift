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
  #expect(keymap.resolve(press(.keyA, [])) == .unbound)
  #expect(keymap.resolve(press(.keyA, .`super`)) == .unbound)
}

@Test("bare arrows are handed to the daemon to encode")
func arrowsAreServerEncoded() {
  // The measured loss: application cursor mode decides between SS3 and CSI, Muster cannot
  // see which is on, and a program that trusts terminfo rejects the wrong one - `less`
  // rings the bell rather than scrolling. The daemon knows, so it encodes these.
  let keymap = Keymap()
  #expect(keymap.resolve(press(.arrowUp, [])) == .serverEncoded("up"))
  #expect(keymap.resolve(press(.arrowDown, [])) == .serverEncoded("down"))
  #expect(keymap.resolve(press(.arrowLeft, [])) == .serverEncoded("left"))
  #expect(keymap.resolve(press(.arrowRight, [])) == .serverEncoded("right"))
}

@Test("a modified arrow keeps its local binding rather than a round trip")
func modifiedArrowsStayLocal() {
  // ⌘← and ⌥← are line and word motion, which are control codes that mean the same thing
  // in every mode. Routing them would buy nothing and cost a socket round trip each.
  let keymap = Keymap()
  #expect(keymap.resolve(press(.arrowLeft, .`super`)) == .text([0x01]))
  #expect(keymap.resolve(press(.arrowLeft, .alt)) == .text([0x1b, UInt8(ascii: "b")]))
  // Nothing bound for shift+arrow, so it takes the local encoder like any other chord.
  #expect(keymap.resolve(press(.arrowUp, .shift)) == .unbound)
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
