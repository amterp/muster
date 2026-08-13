import Foundation

/// A log file several processes append to at once.
///
/// The app and every bridge it spawns share one file, because the questions worth asking
/// span them: a keystroke leaves the app and arrives at a bridge, and two files with two
/// clocks make that a correlation exercise instead of a read.
///
/// Concurrent appends are safe because each record is one `write` to an `O_APPEND`
/// descriptor, which the kernel places at the end atomically, and records are capped below
/// the size where a single write could be split. Fixed key order and sorted fields keep
/// the output diffable.
public final class JSONLinesSink: LogSink, @unchecked Sendable {
  private let fd: Int32
  private let lock = NSLock()

  /// Records longer than this are truncated rather than risking a torn line.
  ///
  /// A split record would corrupt the line either side of it, so losing the tail of one
  /// oversized record is the cheaper failure.
  private static let maximumRecordBytes = 4096

  /// Opens the file, creating it if needed, or returns nil if it cannot be written.
  ///
  /// Logging never takes the process down with it: a diagnostic that can fail the thing
  /// it diagnoses is worse than no diagnostic.
  public init?(path: String) {
    fd = open(path, O_WRONLY | O_APPEND | O_CREAT, 0o644)
    guard fd >= 0 else { return nil }
  }

  deinit {
    close(fd)
  }

  public func write(_ record: LogRecord) {
    var line = Self.encode(record)
    if line.utf8.count > Self.maximumRecordBytes {
      line = String(line.prefix(Self.maximumRecordBytes - 20)) + "…\",\"truncated\":true}"
    }
    let bytes = Array((line + "\n").utf8)
    lock.lock()
    defer { lock.unlock() }
    bytes.withUnsafeBytes { buffer in
      var written = 0
      while written < buffer.count {
        let n = Darwin.write(fd, buffer.baseAddress! + written, buffer.count - written)
        // Nowhere to report a failed log write to, so it is dropped. Retrying forever
        // would hang whatever thread was trying to say something.
        guard n > 0 else { return }
        written += n
      }
    }
  }

  /// Renders one record, with the identifying keys first and the payload after.
  ///
  /// Hand-built rather than `JSONEncoder` so the key order is the one a human reads well:
  /// when, how bad, who, what - then the details, sorted so two runs of the same code
  /// produce the same bytes.
  ///
  /// Both clocks, on every line. `time` is what a person reads; `mono_ns` is what the perf
  /// harness subtracts, and it has to be on the ordinary records rather than on a separate
  /// timing channel, because the hops worth measuring are the ones already being logged.
  static func encode(_ record: LogRecord) -> String {
    var out = "{\"time\":\"\(timestamp.format(record.time))\""
    out += ",\"mono_ns\":\(record.mono)"
    out += ",\"level\":\"\(record.level.rawValue)\""
    out += ",\"process\":\"\(record.process)\""
    out += ",\"pid\":\(record.pid)"
    out += ",\"event\":\(quote(record.event))"
    for key in record.fields.keys.sorted() {
      out += ",\(quote(key)):\(quote(record.fields[key] ?? ""))"
    }
    return out + "}"
  }

  /// ISO 8601 in UTC, to the millisecond, so a log line can be lined up against a wall
  /// clock, a herdr record or a screen recording without arithmetic.
  ///
  /// The format-style value rather than `ISO8601DateFormatter`: the formatter is a class
  /// with mutable state and cannot be shared across the threads that log.
  private static let timestamp = Date.ISO8601FormatStyle(includingFractionalSeconds: true)

  static func quote(_ value: String) -> String {
    var out = "\""
    for scalar in value.unicodeScalars {
      switch scalar {
      case "\"": out += "\\\""
      case "\\": out += "\\\\"
      case "\n": out += "\\n"
      case "\r": out += "\\r"
      case "\t": out += "\\t"
      // Everything else below 0x20 has no short form and must not go through raw.
      case let c where c.value < 0x20:
        out += String(format: "\\u%04x", c.value)
      case let c: out.unicodeScalars.append(c)
      }
    }
    return out + "\""
  }
}
