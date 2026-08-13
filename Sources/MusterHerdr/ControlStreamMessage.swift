import Foundation

/// What Muster can say on a pane's control stream.
///
/// herdr's control stream takes four commands and this covers the three Muster sends
/// (`terminal.release` is a detach, which closing the stream already does). The set is
/// small because herdr's is: everything a client can express about input on this channel
/// is here.
///
/// Encoding lives in this module rather than in the bridge executable so it can be
/// tested - an executable cannot be imported by a test target, and this is the wire
/// format `docs/testing.md` names as the daemon-facing oracle.
public enum ControlStreamMessage: Equatable, Sendable {
  /// Raw bytes for the pane's PTY.
  ///
  /// Raw is not a shortcut: herdr writes these to the PTY untouched, so whatever is here
  /// is exactly what the program receives (`docs/observations/herdr-0.8.0.md` section 5).
  case input([UInt8])

  /// The pane's new grid size, in cells.
  case resize(columns: UInt16, rows: UInt16)

  /// A scroll, as an intent rather than as bytes.
  ///
  /// The daemon answers this against the pane's real modes - encoding a wheel event for
  /// a mouse-reporting program, sending alternate-scroll keys, or moving its own
  /// scrollback. It is the one input-shaped thing Muster does not have to guess about.
  case scroll(direction: ScrollDirection, lines: UInt16)

  public enum ScrollDirection: String, Sendable {
    case up
    case down
  }

  /// The message as herdr's newline-delimited JSON.
  ///
  /// Hand-built rather than `JSONEncoder`-built: the wire format belongs to herdr, and
  /// writing it out literally keeps it readable against their source instead of hidden
  /// behind coding keys.
  public var wireFormat: Data {
    let object: [String: Any] =
      switch self {
      case .input(let bytes):
        ["type": "terminal.input", "bytes": Data(bytes).base64EncodedString()]
      case .resize(let columns, let rows):
        ["type": "terminal.resize", "cols": Int(columns), "rows": Int(rows)]
      case .scroll(let direction, let lines):
        ["type": "terminal.scroll", "direction": direction.rawValue, "lines": Int(lines)]
      }

    guard let json = try? JSONSerialization.data(withJSONObject: object) else { return Data() }
    return json + Data("\n".utf8)
  }
}
