import MusterCore

// Where Muster's vocabulary becomes herdr's. The core produces `PaneIntent`; everything
// that knows what a `terminal.input` envelope looks like lives on this side of the line
// (architecture.md, the backend seam).

extension ControlStreamMessage {
  /// This intent as a control-stream message, or nil if the stream cannot express it.
  ///
  /// The control stream is a raw pipe to the PTY, so it can carry bytes and it can carry a
  /// scroll - which herdr answers itself - and nothing else. Text and named keys are
  /// exactly the intents that need the pane's real modes to encode, which is the thing
  /// this channel does not have.
  init?(_ intent: PaneIntent) {
    switch intent {
    case .input(let bytes):
      self = .input(bytes)
    case .scroll(let direction, let lines):
      self = .scroll(direction: ScrollDirection(direction), lines: lines)
    case .text, .key:
      return nil
    }
  }
}

extension ControlStreamMessage.ScrollDirection {
  init(_ direction: PaneIntent.ScrollDirection) {
    switch direction {
    case .up: self = .up
    case .down: self = .down
    }
  }
}

extension PaneControlChannel: PaneChannel {
  public func deliver(_ intent: PaneIntent) -> Bool {
    guard let message = ControlStreamMessage(intent) else { return false }
    return send(message)
  }

  /// No. Every byte on this channel was encoded by us, against a guess.
  public var encodesServerSide: Bool { false }

  public var channelDescription: String { socketPath }
}
