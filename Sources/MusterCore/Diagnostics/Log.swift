import Foundation

/// What Muster writes down about itself, so that "I just hit a bug" can be answered by
/// reading rather than by reproducing.
///
/// Muster is several processes - the app, and one bridge per pane - and a symptom in one
/// usually has its cause in another. A window that ignores the keyboard can be a bridge
/// that never started, a bridge that started and could not dial back, or a keymap that
/// swallowed the chord, and from the outside those are the same blank stare. So the
/// processes write to one file in one timeline, and the questions become readable.
///
/// Records are one JSON object per line: greppable, and parseable without a tool.
///
/// Off unless a sink is installed. `Log.startFromEnvironment` decides that from
/// `MUSTER_LOG_FILE`, which the app sets for itself and every bridge it spawns.
public enum Log {
  /// Where records go. Nil means logging is off, which is the release default.
  ///
  /// Installed once during startup, before anything else runs, and read from every
  /// thread after that.
  nonisolated(unsafe) private static var sink: LogSink?
  nonisolated(unsafe) private static var minimum: LogLevel = .debug
  nonisolated(unsafe) private static var process: String = "muster"
  private static let lock = NSLock()

  /// Turns logging on for this process.
  public static func install(sink: LogSink, process: String, minimum: LogLevel = .debug) {
    lock.lock()
    defer { lock.unlock() }
    self.sink = sink
    self.process = process
    self.minimum = minimum
  }

  /// Turns logging on if the environment asks for it.
  ///
  /// `MUSTER_LOG_FILE` names the file; `MUSTER_LOG_LEVEL` raises or lowers the bar. The
  /// path is chosen by the shell rather than here, because where logs belong is an
  /// OS question and this layer does not get to have those.
  public static func startFromEnvironment(process: String) {
    let environment = ProcessInfo.processInfo.environment
    guard let path = environment["MUSTER_LOG_FILE"], !path.isEmpty,
      let sink = JSONLinesSink(path: path)
    else { return }
    let level = environment["MUSTER_LOG_LEVEL"].flatMap(LogLevel.init(rawValue:)) ?? .debug
    install(sink: sink, process: process, minimum: level)
  }

  /// Whether records may carry what the user actually typed.
  ///
  /// Off unless `MUSTER_LOG_INPUT=1`, and it stays that way in debug builds too. A log of
  /// every keystroke is a keylogger no matter who wrote it, and this one lands in a file
  /// that gets attached to bug reports. Call sites record the shape of input by default -
  /// which key, how many bytes - and the bytes themselves only when this is on.
  public static let includesInput =
    ProcessInfo.processInfo.environment["MUSTER_LOG_INPUT"] == "1"

  /// Whether anything would come of emitting at this level.
  ///
  /// For call sites where building the fields is itself work worth skipping.
  public static func enabled(_ level: LogLevel = .debug) -> Bool {
    lock.lock()
    defer { lock.unlock() }
    return sink != nil && level >= minimum
  }

  public static func trace(_ event: String, _ fields: [String: String] = [:]) {
    emit(.trace, event, fields)
  }

  public static func debug(_ event: String, _ fields: [String: String] = [:]) {
    emit(.debug, event, fields)
  }

  public static func info(_ event: String, _ fields: [String: String] = [:]) {
    emit(.info, event, fields)
  }

  public static func warn(_ event: String, _ fields: [String: String] = [:]) {
    emit(.warn, event, fields)
  }

  public static func error(_ event: String, _ fields: [String: String] = [:]) {
    emit(.error, event, fields)
  }

  public static func emit(_ level: LogLevel, _ event: String, _ fields: [String: String] = [:]) {
    lock.lock()
    let sink = sink
    let allowed = level >= minimum
    let process = process
    lock.unlock()
    guard let sink, allowed else { return }
    sink.write(
      LogRecord(
        time: Date(), level: level, process: process, pid: getpid(), event: event, fields: fields))
  }
}

public enum LogLevel: String, Sendable, Comparable, CaseIterable {
  /// Per-frame and per-keystroke volume. Off by default: at 60fps it buries everything
  /// that matters.
  case trace
  case debug
  case info
  case warn
  case error

  public static func < (lhs: LogLevel, rhs: LogLevel) -> Bool {
    lhs.rank < rhs.rank
  }

  private var rank: Int {
    switch self {
    case .trace: 0
    case .debug: 1
    case .info: 2
    case .warn: 3
    case .error: 4
    }
  }
}

/// One thing that happened.
///
/// `event` is a dotted name rather than a sentence - `bridge.attach.failed`, not "the
/// bridge could not attach" - so that finding every instance is a grep and not a guess at
/// how it was worded.
public struct LogRecord: Sendable, Equatable {
  public let time: Date
  /// A machine-wide monotonic reading, so two records subtract even across processes.
  /// `time` is for a human lining this up against a wall clock; this is for arithmetic.
  public let mono: UInt64
  public let level: LogLevel
  public let process: String
  public let pid: Int32
  public let event: String
  public let fields: [String: String]

  public init(
    time: Date, mono: UInt64 = MonotonicClock.now(), level: LogLevel, process: String, pid: Int32,
    event: String, fields: [String: String]
  ) {
    self.time = time
    self.mono = mono
    self.level = level
    self.process = process
    self.pid = pid
    self.event = event
    self.fields = fields
  }
}

public protocol LogSink: Sendable {
  func write(_ record: LogRecord)
}
