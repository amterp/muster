import TestSupport
import Testing

@testable import MusterCore

// The input pipeline was moved into the core so it could be tested against fakes at the
// two seams. These are those tests: what reaches a pane, over which channel, in what
// order, and what happens when a channel says no.

@Test("a plain keystroke is encoded locally and goes out as bytes")
func plainKeyTakesTheControlStream() {
  let recorder = SendRecorder()
  let pane = PaneInput(
    channel: FakeChannel(name: "control", recorder: recorder), encoder: FakeEncoder())

  pane.send(press(.keyH, text: "h"))

  #expect(recorder.intents == [.input([0x68])])
  #expect(recorder.channels == ["control"])
}

@Test("an arrow is handed to the daemon, not encoded here")
func arrowsTakeTheServerChannel() {
  // The whole point of the second channel: application cursor mode is invisible from
  // here, so guessing produces bytes a pager rejects.
  let recorder = SendRecorder()
  let pane = PaneInput(
    channel: FakeChannel(name: "control", recorder: recorder),
    serverChannel: FakeChannel(name: "daemon", recorder: recorder, encodesServerSide: true),
    encoder: FakeEncoder())

  pane.send(press(.arrowUp, text: ""))

  #expect(recorder.intents == [.key(name: "up")])
  #expect(recorder.channels == ["daemon"])
}

@Test("with no daemon channel an arrow still reaches the pane, guessed")
func arrowsDegradeRatherThanVanish() {
  // A guessed arrow beats no arrow. The encoder is asked for its best answer instead of
  // the key being dropped.
  let recorder = SendRecorder()
  let pane = PaneInput(
    channel: FakeChannel(name: "control", recorder: recorder), encoder: FakeEncoder())

  pane.send(press(.arrowUp, text: "\u{1b}[A"))

  #expect(recorder.channels == ["control"])
  #expect(recorder.intents == [.input(Array("\u{1b}[A".utf8))])
}

@Test("a daemon that refuses falls back to the control stream")
func serverRefusalFallsBack() {
  // A wedged or departed daemon must not take the keyboard with it.
  let recorder = SendRecorder()
  let pane = PaneInput(
    channel: FakeChannel(name: "control", recorder: recorder),
    serverChannel: FakeChannel(
      name: "daemon", recorder: recorder, encodesServerSide: true, accepts: { _ in false }),
    encoder: FakeEncoder())

  pane.send(press(.arrowUp, text: "\u{1b}[A"))

  #expect(recorder.channels == ["control"])
  #expect(recorder.intents == [.input(Array("\u{1b}[A".utf8))])
}

@Test("typing around an arrow keeps its order across both channels")
func orderSurvivesTwoRoutes() {
  // Bytes reach the PTY through the bridge while a named key goes to the daemon directly,
  // so the two routes have different lengths and can race. `abc<up>def` arriving as
  // `abcdef<up>` is the failure this pins.
  let recorder = SendRecorder()
  let pane = PaneInput(
    channel: FakeChannel(name: "control", recorder: recorder),
    serverChannel: FakeChannel(name: "daemon", recorder: recorder, encodesServerSide: true),
    encoder: FakeEncoder())

  pane.send(press(.keyA, text: "a"))
  pane.send(press(.arrowUp, text: ""))
  pane.send(press(.keyB, text: "b"))

  #expect(recorder.channels == ["control", "daemon", "control"])
  #expect(recorder.intents == [.input([0x61]), .key(name: "up"), .input([0x62])])
}

@Test("a bound chord becomes its own bytes and never reaches the encoder")
func keymapWinsOverTheEncoder() {
  let recorder = SendRecorder()
  let pane = PaneInput(
    channel: FakeChannel(name: "control", recorder: recorder), encoder: FakeEncoder())

  pane.send(press(.backspace, text: "\u{7f}", modifiers: .`super`))

  // 0x15 is unix-line-discard, not the 0x7f the encoder would have produced.
  #expect(recorder.intents == [.input([0x15])])
}

@Test("a paste asks the daemon to encode it, because only it knows the paste mode")
func pasteIsServerEncoded() {
  let recorder = SendRecorder()
  let pane = PaneInput(
    channel: FakeChannel(name: "control", recorder: recorder),
    serverChannel: FakeChannel(name: "daemon", recorder: recorder, encodesServerSide: true),
    encoder: FakeEncoder())

  pane.paste(text: "one\ntwo")

  #expect(recorder.intents == [.text("one\ntwo")])
  #expect(recorder.channels == ["daemon"])
}

@Test("with no daemon a paste still arrives, unfenced")
func pasteDegradesToRawText() {
  // Wrong for several lines - a shell runs them - but sending fence markers to a program
  // that never enabled the mode puts literal `[200~` on its input, which is worse.
  let recorder = SendRecorder()
  let pane = PaneInput(
    channel: FakeChannel(name: "control", recorder: recorder), encoder: FakeEncoder())

  pane.paste(text: "one\ntwo")

  #expect(recorder.intents == [.input(Array("one\ntwo".utf8))])
}

@Test("a keystroke that encodes to nothing sends nothing")
func emptyEncodingsAreNotSent() {
  // Bare modifiers, and every key while an input method is composing. Sending an empty
  // write per modifier press would be constant traffic for no reason.
  let recorder = SendRecorder()
  let pane = PaneInput(
    channel: FakeChannel(name: "control", recorder: recorder), encoder: FakeEncoder())

  pane.send(press(.shiftLeft, text: ""))

  #expect(recorder.intents.isEmpty)
}

@Test("a scroll goes out as an intent rather than as wheel bytes")
func scrollStaysAnIntent() {
  let recorder = SendRecorder()
  let pane = PaneInput(
    channel: FakeChannel(name: "control", recorder: recorder), encoder: FakeEncoder())

  pane.scroll(direction: .up, lines: 3)

  #expect(recorder.intents == [.scroll(direction: .up, lines: 3)])
}

private func press(_ key: Key, text: String, modifiers: Modifiers = []) -> KeyEvent {
  KeyEvent(
    action: .press, key: key, modifiers: modifiers, consumedModifiers: [], text: text,
    unshiftedCodepoint: nil, isComposing: false)
}
