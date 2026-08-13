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

  /// Points this window's keyboard at a daemon-owned pane.
  ///
  /// Returns where the bridge should dial back, or nil if the pane could not be attached -
  /// the core has already said why on stderr and in the log. Nil means the window still
  /// renders and ignores the keyboard, which is the same shape as a bare `muster`.
  public static func attach(paneID: String) -> String? {
    var attach = Muster_AttachPane()
    attach.paneID = paneID
    var request = Muster_Request()
    request.attachPane = attach
    guard case .attached(let attached) = send(request) else { return nil }
    return attached.controlSocketPath
  }

  /// A press, with everything the core needs to decide what it meant.
  ///
  /// Internal, like every generated type: the seam's vocabulary is the shell's business
  /// and stops at this module's edge.
  static func send(
    keyDown key: Muster_KeyEvent,
    wasComposing: Bool,
    committed: String?,
    stillComposing: Bool
  ) {
    var down = Muster_KeyDown()
    down.key = key
    down.wasComposing = wasComposing
    if let committed { down.committed = committed }
    down.stillComposing = stillComposing
    var request = Muster_Request()
    request.keyDown = down
    send(request)
  }

  static func send(keyUp key: Muster_KeyEvent) {
    var up = Muster_KeyUp()
    up.key = key
    var request = Muster_Request()
    request.keyUp = up
    send(request)
  }

  public static func send(text: String) {
    var send = Muster_SendText()
    send.text = text
    var request = Muster_Request()
    request.sendText = send
    Core.send(request)
  }

  public static func paste(text: String) {
    var paste = Muster_Paste()
    paste.text = text
    var request = Muster_Request()
    request.paste = paste
    send(request)
  }

  public static func scroll(direction: String, lines: UInt32) {
    var scroll = Muster_Scroll()
    scroll.direction = direction
    scroll.lines = lines
    var request = Muster_Request()
    request.scroll = scroll
    send(request)
  }

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
  @discardableResult
  private static func send(_ request: Muster_Request) -> Muster_Response.OneOf_Payload? {
    guard let encoded = try? request.serializedBytes() as [UInt8] else {
      FileHandle.standardError.write(
        Data("muster: a request could not be encoded, so the core never saw it.\n".utf8))
      return nil
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
      return nil
    }
    if case .failure(let failure) = decoded.payload {
      FileHandle.standardError.write(Data("muster: \(failure.reason)\n".utf8))
    }
    return decoded.payload
  }

  /// Where the window's chrome hangs, set once by the shell at launch.
  ///
  /// A single observer rather than a broadcast, because there is one window. It becomes a
  /// lookup by pane when composition means there are several, and every event already
  /// names its pane so nothing here has to change shape for that.
  @MainActor public static weak var chrome: PaneChrome?

  /// An event the core sent unasked, already back on the main thread.
  ///
  /// Annotated rather than merely called from a main-actor task, so that touching a view
  /// from here is checked rather than assumed. The hop happens in `coreEventArrived`.
  @MainActor fileprivate static func deliver(_ event: Muster_Event) {
    switch event.payload {
    case .paneTypeable(let typeable):
      info("pane.typeable", ["pane": typeable.paneID])
    case .paneStateChanged(let changed):
      chrome?.apply(paneID: changed.paneID, state: changed.state)
    case .backendHealth(let backend):
      chrome?.apply(health: backend.state, detail: backend.detail)
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
