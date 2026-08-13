import CMuster
import Foundation
import SwiftProtobuf

// The shell's side of the seam. Everything the shell asks of the core goes through here,
// and every request it builds is a message from proto/muster.proto - so a shell operation
// the schema cannot express shows up as a missing message rather than hiding as a call
// into another Swift module (mip/0001-portable-core.md).
//
// Logging is the whole of what crosses today. Deliberately: it is high-volume and its
// failures are visible and cheap, which makes it the right first passenger for a boundary
// nothing has ridden yet.

public enum Core {
  /// Swapped for a recorder in tests. Reads and writes happen during launch and from the
  /// main thread thereafter; a lock here would suggest a concurrency this seam does not
  /// have.
  nonisolated(unsafe) public static var dispatcher: Dispatcher = CoreDispatcher()

  /// Hands the core what it cannot work out for itself, and starts listening for what it
  /// decides on its own.
  ///
  /// `logPath` nil turns logging off, which is what a release build does unless asked.
  public static func start(logPath: String?, process: String = "app") {
    muster_set_event_callback(coreEventArrived)

    var startup = Muster_Startup()
    startup.logPath = logPath ?? ""
    startup.logLevel = ProcessInfo.processInfo.environment["MUSTER_LOG_LEVEL"] ?? ""
    startup.process = process
    var request = Muster_Request()
    request.startup = startup
    send(request)
  }

  /// Whether records may carry what the user actually typed.
  ///
  /// Read here rather than asked of the core, because it is a process-wide privacy switch
  /// and the bridges read the same variable. Off unless `MUSTER_LOG_INPUT=1`, in debug
  /// builds too: a log of every keystroke is a keylogger no matter who wrote it, and this
  /// one lands in a file that gets attached to bug reports.
  public static let includesInput = ProcessInfo.processInfo.environment["MUSTER_LOG_INPUT"] == "1"

  public static func trace(_ event: String, _ fields: [String: String] = [:]) {
    record("trace", event, fields)
  }

  public static func debug(_ event: String, _ fields: [String: String] = [:]) {
    record("debug", event, fields)
  }

  public static func info(_ event: String, _ fields: [String: String] = [:]) {
    record("info", event, fields)
  }

  public static func warn(_ event: String, _ fields: [String: String] = [:]) {
    record("warn", event, fields)
  }

  public static func error(_ event: String, _ fields: [String: String] = [:]) {
    record("error", event, fields)
  }

  private static func record(_ level: String, _ event: String, _ fields: [String: String]) {
    var log = Muster_LogRecord()
    log.level = level
    log.event = event
    log.fields = fields
    var request = Muster_Request()
    request.logRecord = log
    send(request)
  }

  /// Sends a request and reports a refusal to somewhere that is not the log.
  ///
  /// stderr, because the thing most likely to be refused is a log record, and reporting
  /// that failure into the log would be reporting it into the void.
  private static func send(_ request: Muster_Request) {
    guard let encoded = try? request.serializedBytes() as [UInt8] else {
      FileHandle.standardError.write(
        Data("muster: a request could not be encoded, so the core never saw it.\n".utf8))
      return
    }
    let response = dispatcher.dispatch(encoded)
    guard !response.isEmpty, let decoded = try? Muster_Response(serializedBytes: response) else {
      FileHandle.standardError.write(
        Data(
          """
          muster: the core did not answer a request, so whatever it was asking for did not \
          happen. Later requests may still work - this is one failure, not a dead core - \
          but the run log is now incomplete. Its own error is above.

          """.utf8))
      return
    }
    if case .failure(let failure) = decoded.payload {
      FileHandle.standardError.write(Data("muster: \(failure.reason)\n".utf8))
    }
  }

  /// An event the core sent unasked, already back on the main thread.
  fileprivate static func deliver(_ event: Muster_Event) {
    switch event.payload {
    case .paneTypeable(let typeable):
      info("pane.typeable", ["pane": typeable.paneID])
    case nil:
      break
    }
  }
}

/// The C callback, at file scope because `@convention(c)` needs a function that captures
/// nothing.
///
/// Called from whichever core thread noticed - never the main thread - so the bytes are
/// copied here, while they are still valid, and everything else happens after the hop.
/// Nothing in this function may touch AppKit.
private func coreEventArrived(_ bytes: UnsafePointer<UInt8>?, _ length: Int) {
  guard let bytes, length > 0 else { return }
  let copied = Data(bytes: bytes, count: length)
  guard let event = try? Muster_Event(serializedBytes: copied) else { return }
  Task { @MainActor in Core.deliver(event) }
}
