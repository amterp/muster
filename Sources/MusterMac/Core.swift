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
  public static func start(
    logPath: String?, configPath: String? = nil, herdrPath: String? = nil,
    process: String = "app"
  ) {
    muster_set_event_callback(coreEventArrived)

    var startup = Muster_Startup()
    startup.logPath = logPath ?? ""
    startup.configPath = configPath ?? ""
    startup.herdrPath = herdrPath ?? ""
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
  /// Everything that follows is pushed rather than returned: the core opens a socket for
  /// every pane in that pane's tab and publishes the whole view, which is what builds the
  /// surfaces. So the answer here is only whether there is anything to show at all - false
  /// means the window renders nothing and ignores the keyboard, and the core has already said
  /// why on stderr and in the log.
  @discardableResult
  public static func attach(paneID: String) -> Bool {
    var attach = Muster_AttachPane()
    attach.paneID = paneID
    var request = Muster_Request()
    request.attachPane = attach
    guard case .attached = send(request) else { return false }
    return true
  }

  /// Opens the window onto whatever the daemons hold, which is what a bare `muster` means.
  ///
  /// The same shape as `attach`: everything about what ends up on screen is pushed, so the
  /// answer here is only whether there is a session behind this window at all. False means it
  /// renders nothing, and the core has already said why on stderr and in the log.
  @discardableResult
  public static func open() -> Bool {
    var request = Muster_Request()
    request.openWindow = Muster_OpenWindow()
    guard case .ok = send(request) else { return false }
    return true
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

  // What the user can do to a pane. None of these changes a window: they ask the daemon, and
  // the window changes when the view that comes back says it did. An empty pane id means the
  // one this window's keyboard feeds, which is what a keybinding means.

  /// Splits the focused pane, putting the new one beside it (`columns`) or below (`rows`).
  ///
  /// A ratio of zero means the daemon's own default, which is what a keybinding wants; a
  /// drag-to-split would say.
  public static func split(
    axis: String, ratio: Float = 0, daemonID: String = "", paneID: String = ""
  ) {
    var split = Muster_SplitPane()
    split.daemonID = daemonID
    split.paneID = paneID
    split.axis = axis
    split.ratio = ratio
    var request = Muster_Request()
    request.splitPane = split
    send(request)
  }

  /// Makes a tab beside a pane, with one pane in it, and shows it.
  ///
  /// An empty cwd means that pane's own directory, which is what somebody pressing the key
  /// mid-task means. The core resolves it, so the CLI and an agent get the same default.
  public static func createTab(daemonID: String = "", paneID: String = "", cwd: String = "") {
    var create = Muster_CreateTab()
    create.daemonID = daemonID
    create.paneID = paneID
    create.cwd = cwd
    var request = Muster_Request()
    request.createTab = create
    send(request)
  }

  public static func closePane(daemonID: String = "", paneID: String = "") {
    var close = Muster_ClosePane()
    close.daemonID = daemonID
    close.paneID = paneID
    var request = Muster_Request()
    request.closePane = close
    send(request)
  }

  /// Points this window's keyboard at a pane, and tells the daemon somebody looked.
  public static func focus(daemonID: String, paneID: String) {
    var focus = Muster_FocusPane()
    focus.daemonID = daemonID
    focus.paneID = paneID
    var request = Muster_Request()
    request.focusPane = focus
    send(request)
  }

  /// Steps the keyboard one pane along: `next` or `previous`.
  ///
  /// A direction rather than a pane, because the shell does not get to decide what is next -
  /// the order is the tab's tree, which is the daemon's.
  public static func focus(step: String) {
    var relative = Muster_FocusRelative()
    relative.direction = step
    var request = Muster_Request()
    request.focusRelative = relative
    send(request)
  }

  /// Reports whether this window has the OS's focus.
  ///
  /// The one thing about attention that only the shell can see. `done` is `idle` on a pane
  /// nobody looked at, and no daemon can answer that for a window it has no view of - so the
  /// shell says what happened and the core decides what it means.
  public static func windowFocused(_ focused: Bool) {
    var focus = Muster_WindowFocus()
    focus.focused = focused
    var request = Muster_Request()
    request.windowFocus = focus
    send(request)
  }

  /// Moves the line between a region and the one to its right.
  ///
  /// Named by the region on the left, unlike a pane divider: a region has an id that outlives
  /// a drag, where a divider inside a tab is only a position in a tree the daemon may change.
  public static func setRegionBoundary(region: String, ratio: CGFloat) {
    var set = Muster_SetRegionBoundary()
    set.regionID = region
    set.ratio = Float(ratio)
    var request = Muster_Request()
    request.setRegionBoundary = set
    send(request)
  }

  /// Moves one divider, named by the turns from its tab's root.
  public static func setSplitRatio(daemonID: String, tab: String, path: [Bool], ratio: CGFloat) {
    var set = Muster_SetSplitRatio()
    set.daemonID = daemonID
    set.tabID = tab
    set.path = path
    set.ratio = Float(ratio)
    var request = Muster_Request()
    request.setSplitRatio = set
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
      // And into the run log, because a bug report is the log file and a refusal that only
      // reached a terminal nobody kept is a refusal nobody can read. Not for a log record:
      // a refused record reported as a record is a loop.
      if case .logRecord = request.payload {
      } else {
        record("error", "core.refused", ["request": name(of: request), "reason": failure.reason])
      }
    }
    return decoded.payload
  }

  /// Which request was refused, in the schema's own words.
  ///
  /// The reason says what went wrong and this says what the user was trying to do, and a log
  /// line needs both: "no daemon holds a pane called w9:p99" reads very differently under an
  /// attach than under a split.
  private static func name(of request: Muster_Request) -> String {
    switch request.payload {
    case .startup: return "startup"
    case .logRecord: return "log"
    case .attachPane: return "attach_pane"
    case .openWindow: return "open_window"
    case .createTab: return "create_tab"
    case .keyDown: return "key_down"
    case .keyUp: return "key_up"
    case .sendText: return "send_text"
    case .paste: return "paste"
    case .scroll: return "scroll"
    case .splitPane: return "split_pane"
    case .closePane: return "close_pane"
    case .focusPane: return "focus_pane"
    case .focusRelative: return "focus_relative"
    case .setSplitRatio: return "set_split_ratio"
    case .windowFocus: return "window_focus"
    case .setRegionBoundary: return "set_region_boundary"
    case nil: return "(none)"
    }
  }

  /// The window every event reaches, set once by the shell at launch.
  ///
  /// A single observer rather than a broadcast, because there is one window. A second one
  /// makes this a list and changes nothing else: every event already names the pane or region
  /// it is about, so a window that is not showing it drops it.
  @MainActor public static weak var window: MusterWindow?

  /// An event the core sent unasked, already back on the main thread.
  ///
  /// Annotated rather than merely called from a main-actor task, so that touching a view
  /// from here is checked rather than assumed. The hop happens in `coreEventArrived`.
  @MainActor fileprivate static func deliver(_ event: Muster_Event) {
    switch event.payload {
    case .paneTypeable(let typeable):
      info("pane.typeable", ["daemon": typeable.daemonID, "pane": typeable.paneID])
    case .paneStateChanged(let changed):
      window?.apply(
        pane: PaneKey(daemon: changed.daemonID, pane: changed.paneID), state: changed.state)
    case .backendHealth(let backend):
      window?.apply(daemon: backend.daemonID, health: backend.state, detail: backend.detail)
    case .viewChanged(let changed):
      // The shape is already in the log beside this line, written by the core when it
      // published. What is recorded here is that it crossed, and how many surfaces the window
      // was asked for - the part the core cannot see.
      let contents = WindowContents(changed)
      info(
        "view.received",
        [
          "regions": String(contents.regions.count),
          "panes": String(contents.regions.reduce(0) { $0 + ($1.tree?.leaves.count ?? 0) }),
          "keyboard": contents.keyboardPane ?? "",
        ])
      window?.apply(contents)
    case .rosterChanged(let changed):
      let roster = Roster(changed)
      info(
        "roster.received",
        [
          "panes": String(roster.panes.count),
          "on_screen": String(roster.panes.filter(\.onScreen).count),
        ])
      window?.apply(roster)
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
