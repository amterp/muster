import Testing

@testable import MusterCore

// This type exists because of a bug that reached a user: every printable key arrived in
// the pane twice, because `insertText` sent the committed text and the encoder sent the
// keystroke, and nothing decided between them. Typing `hello` produced `hheelllloo`.
//
// The property worth pinning is not any one of these cases - it is that there is exactly
// one outcome per press.

@Test("an ordinary press is the key, even though AppKit also hands back its text")
func plainKeyIsNotAlsoText() {
  // The regression. AppKit calls insertText("h") for a plain `h` with no input method
  // involved, so `committed` is non-nil here - and it must still lose to the keystroke.
  #expect(
    CompositionArbiter.outcome(wasComposing: false, committed: "h", stillComposing: false)
      == .sendKey)
}

@Test("a finished composition is its text, not the key that finished it")
func commitSendsText() {
  // option+e then e: the second press ends the composition and produces `é`, which has no
  // relationship to the `e` that was pressed.
  #expect(
    CompositionArbiter.outcome(wasComposing: true, committed: "é", stillComposing: false)
      == .sendText("é"))
}

@Test("a press that starts or continues a composition reaches nothing")
func composingSwallowsTheKey() {
  // option+e: a preedit opens and the pane must not see the keystroke behind it.
  #expect(
    CompositionArbiter.outcome(wasComposing: false, committed: nil, stillComposing: true)
      == .sendNothing)
  // A candidate selection mid-composition, where the method commits and re-marks in one
  // press: still the method's business.
  #expect(
    CompositionArbiter.outcome(wasComposing: true, committed: "か", stillComposing: true)
      == .sendNothing)
}

@Test("a composition that commits nothing falls back to the keystroke")
func emptyCommitIsNotSwallowed() {
  // Escaping out of a preedit ends the composition without producing text. Treating that
  // as `.sendText("")` would silently eat the key.
  #expect(
    CompositionArbiter.outcome(wasComposing: true, committed: "", stillComposing: false)
      == .sendKey)
  #expect(
    CompositionArbiter.outcome(wasComposing: true, committed: nil, stillComposing: false)
      == .sendKey)
}
