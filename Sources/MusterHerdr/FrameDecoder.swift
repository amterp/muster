import Foundation

/// One frame off a pane's data plane.
public struct PaneFrame: Equatable, Sendable {
  /// The ANSI the daemon rendered for this pane's screen. Already encoded for a
  /// terminal, and already stripped of the inner program's mode changes.
  public let bytes: Data
  /// Whether this repaints the whole screen rather than a diff against the last one.
  public let isFull: Bool
  /// Monotonic per pane. The data plane has sequence numbers even though the control
  /// plane does not, so staleness here is detectable.
  public let sequence: Int
}

/// What a decoded line off the stream turned out to be.
public enum PaneStreamEvent: Equatable, Sendable {
  case frame(PaneFrame)
  /// The daemon hung up on this pane.
  case closed(reason: String?)
}

/// Turns a pane's newline-delimited JSON stream back into frames.
///
/// Pure, and deliberately so. This is the only decidable logic in an otherwise
/// I/O-shaped path, and keeping it here rather than inside the bridge executable is what
/// lets the awkward parts be tested: a 35 KB repaint split across arbitrary reads, a
/// partial line at the end of a chunk, garbage between good frames.
public struct FrameDecoder: Sendable {
  private var pending = Data()

  public init() {}

  /// Feeds a chunk of stream and returns whatever completed inside it.
  ///
  /// Anything past the last newline is held: frames routinely arrive split across reads,
  /// and half a JSON object decodes to nothing rather than to something wrong.
  public mutating func consume(_ chunk: Data) -> [PaneStreamEvent] {
    pending.append(chunk)
    var events: [PaneStreamEvent] = []

    while let newline = pending.firstIndex(of: UInt8(ascii: "\n")) {
      let line = pending[pending.startIndex..<newline]
      pending = pending[pending.index(after: newline)...]
      if let event = Self.decode(line) { events.append(event) }
    }

    return events
  }

  /// Decodes one line, or nothing.
  ///
  /// A line we cannot read is skipped rather than fatal. herdr's API is explicitly
  /// unstable, and an unknown message type is a thing it may add next week - dropping
  /// the pane's whole stream over one is a worse failure than ignoring it.
  static func decode(_ line: Data) -> PaneStreamEvent? {
    guard
      let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
      let type = object["type"] as? String
    else { return nil }

    switch type {
    case "terminal.frame":
      guard
        let encoded = object["bytes"] as? String,
        let bytes = Data(base64Encoded: encoded)
      else { return nil }
      return .frame(
        PaneFrame(
          bytes: bytes,
          isFull: object["full"] as? Bool ?? false,
          sequence: object["seq"] as? Int ?? 0))
    case "terminal.closed":
      return .closed(reason: object["reason"] as? String)
    default:
      return nil
    }
  }
}
