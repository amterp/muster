import AppKit
import CMuster
import Foundation
import MusterRenderer
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
    logPath: String?, configPath: String? = nil, daemonPath: String? = nil,
    statePath: String? = nil, daemonConfigPath: String? = nil, paneNamesPath: String? = nil,
    commandSocketPath: String? = nil, commandsPath: String? = nil, cachePath: String? = nil,
    fresh: Bool = false, process: String = "app"
  ) {
    muster_set_event_callback(coreEventArrived)

    var startup = Muster_Startup()
    startup.logPath = logPath ?? ""
    startup.configPath = configPath ?? ""
    startup.daemonPath = daemonPath ?? ""
    startup.statePath = statePath ?? ""
    startup.daemonConfigPath = daemonConfigPath ?? ""
    startup.paneNamesPath = paneNamesPath ?? ""
    startup.commandSocketPath = commandSocketPath ?? ""
    startup.commandsPath = commandsPath ?? ""
    startup.cachePath = cachePath ?? ""
    startup.fresh = fresh
    startup.locale = platformLocale() ?? ""
    startup.logLevel = ProcessInfo.processInfo.environment["MUSTER_LOG_LEVEL"] ?? ""
    startup.process = process
    var request = Muster_Request()
    request.startup = startup
    send(request)
    watchForTermination()
  }

  /// Tells the core when this process is going away.
  ///
  /// The one thing about its own lifetime the core cannot see: it is a dylib, and a dylib does
  /// not get to notice its host terminating. What it does about it - handing every pane back at
  /// the size its daemon draws it at, because the daemon holds a pane at its controller's
  /// geometry and never lets go - has to happen while this window's bridges are still alive to
  /// relay it, so this is sent synchronously and the core answers when it is done.
  ///
  /// Registered here rather than in the app delegate, because this is the shell's one edge onto
  /// the core and the delegate is deliberately a thing that decides nothing.
  ///
  /// `willTerminate` rather than a window closing: quitting is what leaves a session behind,
  /// and ⌘Q does not close windows on the way out.
  private static func watchForTermination() {
    NotificationCenter.default.addObserver(
      forName: NSApplication.willTerminateNotification, object: nil, queue: .main
    ) { _ in
      quitting(closeSessions: closingSessions)
    }
  }

  /// Whether this quit is the one that ends the sessions too.
  ///
  /// Set by the menu item that asks, and read by the observer above, because between the two
  /// is `NSApp.terminate` and there is nothing to carry a value through it. It is not a second
  /// way to quit: there is one path out and this says which of two things it does on the way.
  nonisolated(unsafe) private static var closingSessions = false

  /// Ends the sessions this window is attached to as part of quitting, rather than leaving
  /// them running.
  ///
  /// Answered when the daemons have stopped, which is why it is worth setting a flag rather
  /// than sending a second message: the shell is holding its own termination open on the reply
  /// either way, and one path out is what stops a quit that half-happened.
  public static func closeSessionsOnQuit() {
    closingSessions = true
  }

  static func quitting(closeSessions: Bool) {
    var quitting = Muster_Quitting()
    quitting.closeSessions = closeSessions
    var request = Muster_Request()
    request.quitting = quitting
    send(request)
  }

  /// Every machine this window is attached to, and what each is holding.
  ///
  /// Asked rather than accumulated from events, because it is read once at the moment somebody
  /// is deciding whether to end them - and a picture assembled from four event streams would be
  /// a second answer that can disagree with `muster window`.
  public static func machines() -> [Machine] {
    var request = Muster_Request()
    request.readWindow = Muster_ReadWindow()
    guard case .window(let answer) = send(request) else { return [] }
    return answer.daemons.map {
      Machine(
        daemon: $0.daemonID, host: $0.host, socket: $0.socket,
        startedByMuster: $0.startedByMuster, panes: Int($0.panes), directories: $0.directories)
    }
  }

  /// One machine this window is attached to.
  public struct Machine: Equatable, Sendable {
    public let daemon: String
    /// Where it runs, or empty for this machine.
    public let host: String
    public let socket: String
    public let startedByMuster: Bool
    public let panes: Int
    public let directories: [String]

    public init(
      daemon: String, host: String, socket: String, startedByMuster: Bool, panes: Int,
      directories: [String]
    ) {
      self.daemon = daemon
      self.host = host
      self.socket = socket
      self.startedByMuster = startedByMuster
      self.panes = panes
      self.directories = directories
    }
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

  /// Scrolls one named pane, which is the pane the pointer was over rather than the focused
  /// one. Both ids, because two daemons hand out the same pane ids.
  public static func scroll(daemonID: String, paneID: String, direction: String, delta: Double) {
    var scroll = Muster_Scroll()
    scroll.daemonID = daemonID
    scroll.paneID = paneID
    scroll.direction = direction
    scroll.delta = delta
    var request = Muster_Request()
    request.scroll = scroll
    send(request)
  }

  // What the user can do to a pane. None of these changes a window: they ask the daemon, and
  // the window changes when the view that comes back says it did. An empty pane id means the
  // one this window's keyboard feeds, which is what a keybinding means.

  /// Splits the focused pane, putting the new one on the named side of it.
  ///
  /// All four sides, whatever the daemon behind it offers: which of them cost one request and
  /// which cost two is the core's problem and never this one's.
  ///
  /// A ratio of zero means the daemon's own default, which is what a keybinding wants; a
  /// drag-to-split would say.
  public static func split(
    side: String, ratio: Float = 0, daemonID: String = "", paneID: String = ""
  ) {
    var split = Muster_SplitPane()
    split.daemonID = daemonID
    split.paneID = paneID
    split.side = side
    split.ratio = ratio
    // Said outright, because the field defaults to false and false is what a script means. This
    // is a chord or a menu item: somebody made a pane and is looking at it, so the keyboard goes
    // there. Left unset, every split in the window would leave the cursor behind.
    split.takeFocus = true
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
    // Said outright for the same reason a split says it, and with the same consequence if it
    // were left alone: the field defaults to false because false is what a script means, and
    // cmd+T is somebody making a tab to work in.
    create.takeFocus = true
    var request = Muster_Request()
    request.createTab = create
    send(request)
  }

  /// Calls a pane what somebody wants to call it.
  ///
  /// An empty name takes the name away, leaving the pane called after its directory again -
  /// one spelling for "no name" rather than two that draw the same.
  public static func renamePane(name: String, daemonID: String = "", paneID: String = "") {
    var rename = Muster_RenamePane()
    rename.daemonID = daemonID
    rename.paneID = paneID
    rename.name = name
    var request = Muster_Request()
    request.renamePane = rename
    send(request)
  }

  /// Calls a tab what somebody wants to call it.
  ///
  /// An empty tab id means the tab the keyboard's pane is in, which is what a menu item means -
  /// there is no other way for one to point at a tab. No machine goes with it: a Muster tab may
  /// hold panes on two, and the core renames every half of one.
  public static func renameTab(name: String, tabID: String = "") {
    var rename = Muster_RenameTab()
    rename.tabID = tabID
    rename.name = name
    var request = Muster_Request()
    request.renameTab = rename
    send(request)
  }

  /// Grows the focused pane against its neighbour, by the daemon's own step.
  ///
  /// A direction rather than a divider: which one moves is a question about a tree the person
  /// pressing the key is not looking at, and only the daemon holds the rectangles to answer it.
  /// `cell` is how big one cell is on the surface the chord happened on and `region` how big
  /// the whole region is, both in points and both needed to read a `resize_step`: the daemon
  /// moves a divider by a share of what it divides, so a distance has nothing to be a share of
  /// until something measures the region. Omitting either means "could not measure", which the
  /// core answers with the daemon's own step rather than a guess.
  public static func resize(
    direction: String, amount: Float = 0, cell: (width: Float, height: Float)? = nil,
    region: (width: Float, height: Float)? = nil,
    daemonID: String = "", paneID: String = ""
  ) {
    var resize = Muster_ResizePane()
    resize.daemonID = daemonID
    resize.paneID = paneID
    resize.direction = direction
    resize.amount = amount
    resize.cellWidth = cell?.width ?? 0
    resize.cellHeight = cell?.height ?? 0
    resize.regionWidth = region?.width ?? 0
    resize.regionHeight = region?.height ?? 0
    var request = Muster_Request()
    request.resizePane = resize
    send(request)
  }

  /// Makes the focused pane fill its tab, or puts it back.
  public static func zoom(daemonID: String = "", paneID: String = "") {
    var zoom = Muster_ZoomPane()
    zoom.daemonID = daemonID
    zoom.paneID = paneID
    var request = Muster_Request()
    request.zoomPane = zoom
    send(request)
  }

  /// What Muster should look like, as the config file decided.
  ///
  /// A value of the shell's own rather than the generated message, on the same terms as
  /// `bindings()`: the seam's vocabulary stops at this module's edge. Split in two here
  /// because two different pieces of code paint it - the renderer fills a pane, and Muster
  /// draws the line between two of them - which is a fact about the shell and the reason the
  /// core answers with one message rather than two.
  public struct Appearance: Sendable {
    /// What a pane looks like, in the renderer seam's vocabulary.
    public let pane: MusterRenderer.Appearance
    /// What Muster paints around a pane rather than inside it.
    public let chrome: Chrome
  }

  /// The colours no renderer paints: the line between two regions, the ring saying which pane
  /// has the keyboard, and the five agent states.
  ///
  /// Grouped rather than seven fields on `Appearance`, because they go to one place - the
  /// window's own chrome - and arrive together on one event. Every one is nil for "the file
  /// said nothing", and every one has a different answer to what that means: the platform's
  /// separator, the platform's accent, and the legend `PaneAppearance` holds.
  public struct Chrome: Sendable {
    public let divider: String?
    public let focusRing: String?
    public let agents: AgentColors

    public static let none = Chrome(divider: nil, focusRing: nil, agents: AgentColors())
  }

  /// The five agent states as `#rrggbb`, or nil each for the one Muster ships.
  public struct AgentColors: Sendable {
    public var working: String?
    public var blocked: String?
    public var done: String?
    public var idle: String?
    public var unknown: String?

    /// Which states a person repainted, for the run log. The states rather than the values,
    /// because what a reader is asking when a colour looks wrong is whether the file reached
    /// Muster at all - and six hex triples on a log line answers that no better than five
    /// words do.
    var described: String {
      let named = [
        ("working", working), ("blocked", blocked), ("done", done), ("idle", idle),
        ("unknown", unknown),
      ].filter { $0.1 != nil }.map(\.0)
      return named.isEmpty ? "(default)" : named.joined(separator: " ")
    }
  }

  /// What Muster paints itself with.
  ///
  /// Everything absent when the core did not answer, which is a core that failed to start -
  /// the caller falls back to the renderer's and the platform's own rather than refusing to
  /// draw.
  public static func appearance() -> Appearance {
    var request = Muster_Request()
    request.readAppearance = Muster_ReadAppearance()
    guard case .appearance(let answer) = send(request) else {
      return Appearance(pane: MusterRenderer.Appearance(), chrome: .none)
    }
    return read(answer)
  }

  /// One decoding for the launch-time read and the reload event, so the two cannot drift.
  static func read(_ answer: Muster_Appearance) -> Appearance {
    // Empty is how a string field says "nothing was named", so it becomes nil rather than an
    // empty family name or a colour of no digits.
    let named = { (value: String) in value.isEmpty ? nil : value }
    return Appearance(
      pane: MusterRenderer.Appearance(
        fontFamily: named(answer.fontFamily),
        // Zero likewise, and it cannot collide with a size somebody meant: the core refuses
        // anything below one point.
        fontSize: answer.fontSize == 0 ? nil : answer.fontSize,
        background: named(answer.background),
        foreground: named(answer.foreground),
        cursor: named(answer.cursor),
        cursorText: named(answer.cursorText),
        selectionBackground: named(answer.selectionBackground),
        selectionForeground: named(answer.selectionForeground),
        bold: named(answer.bold),
        palette: answer.palette,
        cursorStyle: MusterRenderer.Appearance.CursorStyle(rawValue: answer.cursorStyle),
        cursorBlink: answer.hasCursorBlink ? answer.cursorBlink : nil,
        panePadding: answer.hasPanePadding ? answer.panePadding : nil
      ),
      chrome: Chrome(
        divider: named(answer.dividerColor),
        focusRing: named(answer.focusRingColor),
        agents: AgentColors(
          working: named(answer.agentColors.working),
          blocked: named(answer.agentColors.blocked),
          done: named(answer.agentColors.done),
          idle: named(answer.agentColors.idle),
          unknown: named(answer.agentColors.unknown))))
  }

  /// How big the window should be, and whether it should be full-screen.
  ///
  /// A value of the shell's own rather than the generated message, on the same terms as
  /// `Appearance`. `rect` is nil for a window that has never settled anywhere, which is a
  /// first launch and the caller's cue to open wherever it would have.
  public struct WindowFrame: Sendable {
    public let rect: NSRect?
    public let fullScreen: Bool
  }

  /// Where this window should open, given the screens this machine has.
  ///
  /// Asked rather than waited for, because the answer is needed before the window is on screen
  /// and an event arrives a run-loop turn later - the same split `appearance()` makes. The
  /// screens go with the question because only the shell can ask the platform for them; where
  /// the window lands is the core's answer, already fitted to them.
  ///
  /// Nothing at all when the core did not answer, which is a core that failed to start. The
  /// window then opens where a first launch does rather than not at all.
  public static func windowFrame(screens: [NSRect]) -> WindowFrame {
    var read = Muster_ReadWindowFrame()
    read.screens = screens.map(rect)
    var request = Muster_Request()
    request.readWindowFrame = read
    guard case .windowFrame(let answer) = send(request) else {
      return WindowFrame(rect: nil, fullScreen: false)
    }
    return WindowFrame(
      rect: answer.hasRect
        ? NSRect(
          x: answer.rect.x, y: answer.rect.y, width: answer.rect.width, height: answer.rect.height)
        : nil,
      fullScreen: answer.fullScreen)
  }

  /// Says where the window has settled, so the next launch can put it back.
  ///
  /// Sent through `WindowFrameSender` rather than from here, because a drag produces one of
  /// these per frame of animation and each one ends in a file write. This builds the request;
  /// the sender decides when it goes.
  static func setWindowFrame(rect: NSRect?, fullScreen: Bool) -> Muster_Request {
    var frame = Muster_WindowFrame()
    if let rect { frame.rect = self.rect(rect) }
    frame.fullScreen = fullScreen
    var set = Muster_SetWindowFrame()
    set.frame = frame
    var request = Muster_Request()
    request.setWindowFrame = set
    return request
  }

  private static func rect(_ rect: NSRect) -> Muster_WindowRect {
    var wire = Muster_WindowRect()
    wire.x = Double(rect.origin.x)
    wire.y = Double(rect.origin.y)
    wire.width = Double(rect.size.width)
    wire.height = Double(rect.size.height)
    return wire
  }

  /// Tells the core what this machine makes of the font family the config named.
  ///
  /// A report rather than a question: the shell can see whether the font is here and the core
  /// decides whether that is worth telling anybody. Sent at launch and again on every reload, so
  /// a corrected family clears the problem the way a corrected config does - which is the only
  /// acknowledgement a fix ever gets.
  public static func reportFontFamily(_ family: String?) {
    var report = Muster_ReportFontFamily()
    report.family = family ?? ""
    let found = InstalledFont.look(up: report.family)
    report.found = found.found
    report.monospaced = found.monospaced
    var request = Muster_Request()
    request.reportFontFamily = report
    send(request)
  }

  /// Makes the text in every pane bigger or smaller, or puts it back.
  ///
  /// A direction rather than a size, matching `toggleSidebar`: what a chord means is "one more
  /// than whatever I have", and the shell does not hold what it has.
  public static func adjustFontSize(_ change: String) {
    var adjust = Muster_AdjustFontSize()
    adjust.change = change
    var request = Muster_Request()
    request.adjustFontSize = adjust
    send(request)
  }

  /// Reads the config file again, and makes the window match it.
  ///
  /// Nothing comes back on this call: what changed arrives as the events the core sends, the
  /// same way a view does. So the file watcher and the menu item are one path rather than two.
  public static func reloadConfig() {
    var request = Muster_Request()
    request.reloadConfig = Muster_ReloadConfig()
    send(request)
  }

  public static func toggleSidebar() {
    var request = Muster_Request()
    request.toggleSidebar = Muster_ToggleSidebar()
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

  /// Reports that nothing is painting a pane any more.
  ///
  /// An observation rather than a request. The shell is the only thing that can see its own
  /// subprocess end, and the core is the only thing that can find out what that means - most
  /// often that the daemon no longer holds the pane, which it does not always announce.
  public static func bridgeExited(daemonID: String, paneID: String, processAlive: Bool) {
    var exited = Muster_BridgeExited()
    exited.daemonID = daemonID
    exited.paneID = paneID
    exited.processAlive = processAlive
    var request = Muster_Request()
    request.bridgeExited = exited
    send(request)
  }

  /// One of Muster's actions and the chord asking for it.
  ///
  /// The core's vocabulary, in the shell's own type: the generated messages stop at this
  /// module's edge like every other one, so a menu builder never sees a protobuf.
  public struct Binding: Sendable {
    public let action: String
    public let key: String
    public let modifiers: [String]

    public init(action: String, key: String, modifiers: [String]) {
      self.action = action
      self.key = key
      self.modifiers = modifiers
    }
  }

  /// Every action and its chord, as the config file left them.
  ///
  /// Empty when the core will not answer, which is a menu with no pane shortcuts rather than
  /// a launch that fails - the core has already said why in the log, and a window somebody
  /// can click around is worth more than one that refuses to open.
  public static func bindings() -> [Binding] {
    var request = Muster_Request()
    request.readBindings = Muster_ReadBindings()
    guard case .bindings(let answer) = send(request) else { return [] }
    return read(answer)
  }

  /// One decoding for the launch-time read and the reload event, on the same terms as
  /// `read(_:)` for appearance.
  static func read(_ answer: Muster_Bindings) -> [Binding] {
    answer.bindings.map { Binding(action: $0.action, key: $0.key, modifiers: $0.modifiers) }
  }

  /// What a search found, which is everything the find bar draws.
  public struct Findings: Equatable, Sendable {
    public let total: UInt32
    /// Which match is selected, counting from one. Zero when nothing matched.
    public let selected: UInt32
    /// How many rows the core managed to look at.
    public let rowsSearched: UInt32
    /// Whether the pane holds history the search never reached.
    public let truncated: Bool

    /// Nothing typed, so nothing found. What an empty field shows.
    public static let none = Findings(total: 0, selected: 0, rowsSearched: 0, truncated: false)
  }

  /// Looks for text in the pane the keyboard is on, and lands on the first match.
  ///
  /// Sent per keystroke: the needle is the whole question every time, never something added
  /// to. A core that refuses answers `nil`, which the bar draws as no matches rather than as
  /// an error - the reason is already in the log, and a search box is a poor place to report
  /// a daemon problem.
  public static func find(needle: String, daemonID: String = "", paneID: String = "")
    -> Findings?
  {
    var find = Muster_Find()
    find.daemonID = daemonID
    find.paneID = paneID
    find.needle = needle
    var request = Muster_Request()
    request.find = find
    guard case .findings(let answer) = send(request) else { return nil }
    return read(answer)
  }

  /// Goes to the next match, or the previous one, and lands on it.
  public static func stepFind(forward: Bool) -> Findings? {
    var step = Muster_FindStep()
    step.direction = forward ? "next" : "previous"
    var request = Muster_Request()
    request.findStep = step
    guard case .findings(let answer) = send(request) else { return nil }
    return read(answer)
  }

  /// Forgets the search, which is what closing the find bar means.
  public static func endFind() {
    var request = Muster_Request()
    request.endFind = Muster_EndFind()
    send(request)
  }

  static func read(_ answer: Muster_Findings) -> Findings {
    Findings(
      total: answer.total, selected: answer.selected,
      rowsSearched: answer.rowsSearched, truncated: answer.truncated)
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

  /// Steps the keyboard one tab along: `next` or `previous`.
  ///
  /// The other axis to stepping panes. That one walks what the window is showing; this walks
  /// every tab every attached daemon holds, which is the only keyboard path to a tab no region
  /// has on screen.
  public static func focus(tabStep: String) {
    var relative = Muster_FocusTabRelative()
    relative.direction = tabStep
    var request = Muster_Request()
    request.focusTabRelative = relative
    send(request)
  }

  /// Puts the keyboard on the pane at a place in the window's pane order, counting from one.
  ///
  /// The place is the core's numbering, which is the number the sidebar draws beside the row -
  /// so ⌘3 and the third numbered row are one pane. A pane in a tab nothing is showing brings
  /// that tab on screen, which is why there is no numbered chord for a tab.
  public static func focus(panePlace: Int) {
    var at = Muster_FocusPaneAt()
    at.place = UInt32(max(0, panePlace))
    var request = Muster_Request()
    request.focusPaneAt = at
    send(request)
  }

  /// Takes back a numbered chord that named a tab, because the gesture is over.
  ///
  /// What letting go of the modifier means under `numbered_chords = "tab_then_pane"`. The core
  /// decides what that costs; this side only decides that the hand has finished, which is the
  /// one half of it only a shell can see.
  public static func endNumberedChord() {
    var request = Muster_Request()
    request.endNumberedChord = Muster_EndNumberedChord()
    send(request)
  }

  /// Puts one pane where another is, which is what dropping a row on a row means.
  ///
  /// Both ends are named because a drag names two panes by definition. Whether this becomes an
  /// exchange or a move into another tab is the core's to decide from where they are - the
  /// shell knows which rows were involved and nothing about the tree they sit in.
  /// Closes the tab the window's keyboard is in, and every pane in it.
  ///
  /// Naming no tab means the keyboard's, which is what a menu item means. Unlike going to a tab,
  /// closing one has a sensible "the one I am already in" - that is exactly what somebody
  /// picking this is asking for.
  public static func closeTab() {
    var request = Muster_Request()
    request.closeTab = Muster_CloseTab()
    send(request)
  }

  /// Takes the pane the window's keyboard is on into a tab of its own.
  ///
  /// The same request a drag sends, with the other destination set. Naming no pane means the
  /// keyboard's, which is what a menu item means and what the core reads an empty id as - a drag
  /// says which pane because the row it started on knows, and a menu item has no row.
  public static func movePaneToNewTab() {
    var arrange = Muster_ArrangePane()
    arrange.newTab = true
    var request = Muster_Request()
    request.arrangePane = arrange
    send(request)
  }

  public static func arrange(pane: PaneKey, onto: PaneKey) {
    var arrange = Muster_ArrangePane()
    arrange.daemonID = pane.daemon
    arrange.paneID = pane.pane
    arrange.ontoPaneID = onto.pane
    var request = Muster_Request()
    request.arrangePane = arrange
    send(request)
  }

  /// Puts a pane into a tab, which is what dropping its row on a caption means.
  ///
  /// The one arrangement that may cross machines: a tab is Muster's grouping rather than a
  /// daemon's, so a pane joining one from another machine changes which tab it is in and moves
  /// no process anywhere.
  public static func arrange(pane: PaneKey, intoTab tab: String) {
    var arrange = Muster_ArrangePane()
    arrange.daemonID = pane.daemon
    arrange.paneID = pane.pane
    arrange.tabID = tab
    var request = Muster_Request()
    request.arrangePane = arrange
    send(request)
  }

  /// Brings a named tab on screen, which is what clicking its caption means.
  ///
  /// Named rather than numbered: a click knows which tab it hit, and the numbers name panes.
  /// No machine goes with it - a Muster tab may hold panes on two, so it belongs to neither.
  public static func focus(tab: String) {
    var focus = Muster_FocusTab()
    focus.tabID = tab
    var request = Muster_Request()
    request.focusTab = focus
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

  // Moving a divider is deliberately not here. It is the one request a drag produces a hundred
  // times a second, so it goes through `SplitRatioSender` - which keeps one in flight and sends
  // the latest position when that returns, rather than blocking the main thread per frame.

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
    case .readBindings: return "read_bindings"
    case .readAppearance: return "read_appearance"
    case .readWindow: return "read_window"
    case .readPane: return "read_pane"
    case .readWindowFrame: return "read_window_frame"
    case .setWindowFrame: return "set_window_frame"
    case .reportFontFamily: return "report_font_family"
    // The kind, never the text, for the reason a find needle is never logged: what somebody
    // types into their own terminal is theirs.
    case .sendToPane: return "send_to_pane"
    case .adjustFontSize: return "adjust_font_size"
    case .reloadConfig: return "reload_config"
    case .bridgeExited: return "bridge_exited"
    case .resizePane: return "resize_pane"
    case .zoomPane: return "zoom_pane"
    case .toggleSidebar: return "toggle_sidebar"
    case .focusTabRelative: return "focus_tab_relative"
    case .focusPaneAt: return "focus_pane_at"
    case .focusTab: return "focus_tab"
    case .arrangePane: return "arrange_pane"
    // The kind of request, never the name it carried. A name is text a person wrote about
    // their own work, and this line ends up in a file destined for a bug report.
    case .renamePane: return "rename_pane"
    case .renameTab: return "rename_tab"
    case .closeTab: return "close_tab"
    // The kind, never the needle, for the reason above and more sharply: what somebody is
    // looking for in their own terminal is the most private thing this seam carries.
    case .find: return "find"
    case .findStep: return "find_step"
    case .endFind: return "end_find"
    case .endNumberedChord: return "end_numbered_chord"
    case .quitting: return "quitting"
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
    case .appearanceChanged(let changed):
      let appearance = read(changed.appearance)
      info(
        "appearance.received",
        [
          "divider": appearance.chrome.divider ?? "(platform)",
          "focus_ring": appearance.chrome.focusRing ?? "(accent)",
          "agents": appearance.chrome.agents.described,
        ])
      window?.apply(appearance: appearance)
      // The family may have changed with everything else, and the answer to whether this
      // machine has it can only be looked up here.
      reportFontFamily(appearance.pane.fontFamily)
    case .bindingsChanged(let changed):
      let bindings = read(changed.bindings)
      info("bindings.received", ["actions": String(bindings.count)])
      window?.apply(bindings: bindings)
    case .rosterChanged(let changed):
      let roster = Roster(changed)
      info(
        "roster.received",
        [
          "panes": String(roster.panes.count),
          "on_screen": String(roster.panes.filter(\.onScreen).count),
        ])
      window?.apply(roster)
    case .problemsChanged(let changed):
      let problems = changed.problems.map {
        Problem(key: $0.key, severity: Problem.Severity($0.severity), detail: $0.detail)
      }
      // Counted rather than quoted. The detail is already in the core's own warning beside
      // this line, and a run log that repeated every refusal twice would be harder to read
      // for no new fact.
      info(
        "problems.received",
        [
          "count": String(problems.count),
          "errors": String(problems.filter { $0.severity == .error }.count),
        ])
      window?.apply(problems: problems)
    case .attentionChanged(let changed):
      // Straight on, because the decision was made before it crossed: the core holds the
      // unread set, knows what this window is showing, and holds the file's answer about
      // which states are worth interrupting for. Filtering here would be re-deriving it
      // from two halves the shell does not have.
      PaneNotifier.shared.apply(
        daemon: changed.daemonID, pane: changed.paneID, state: changed.state,
        label: changed.label, subtitle: changed.subtitle)
    case .presentationChanged(let changed):
      let presentation = Presentation(sidebar: changed.sidebar)
      info("presentation.received", ["sidebar": String(presentation.sidebar)])
      window?.apply(presentation: presentation)
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
