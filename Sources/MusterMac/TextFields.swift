import AppKit

// What Muster's own text fields share: a banner's reply and a conversation's message box. Each
// matches the chords that belong to it rather than leaving them to the menu (`in_text_field` in
// the core's keymap), and each lets an input method finish before Escape means anything else.

extension Core.Binding {
  /// Whether a key press, as AppKit spells its key, is this chord. Only the four modifiers a
  /// chord can carry are compared.
  func matches(key: String, modifiers: NSEvent.ModifierFlags) -> Bool {
    guard !self.key.isEmpty else { return false }
    let pressed = modifiers.intersection([.shift, .control, .option, .command])
    return menuKeyEquivalent(forKeyNamed: self.key) == key
      && pressed == menuModifiers(self.modifiers)
  }

  /// The same for an event, read off its unmodified characters.
  func matches(_ event: NSEvent) -> Bool {
    matches(key: event.charactersIgnoringModifiers ?? "", modifiers: event.modifierFlags)
  }
}

extension NSTextView {
  /// Ends an input method's composition here, discarding what was being composed, as Escape
  /// does in a composing input method: told first, so one that keeps a composition of its own
  /// drops it, and then the marked text replaced with nothing in case it kept none. False when
  /// nothing was being composed.
  func endComposition() -> Bool {
    guard hasMarkedText() else { return false }
    inputContext?.discardMarkedText()
    if hasMarkedText() {
      insertText("", replacementRange: markedRange())
    }
    return true
  }
}
