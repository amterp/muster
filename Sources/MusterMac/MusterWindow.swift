import AppKit
import MusterRenderer

/// The window, and everything hanging under it.
///
/// The shell's whole contribution to what is on screen. It holds no truth: every region,
/// every pane and which one has the keyboard arrive from the core as a whole view, and this
/// applies it. What it does own is what only an OS layer can - a real window, real surfaces,
/// the responder chain, and a title.
///
/// A class rather than top-level code in the executable, because everything here has been
/// wrong at least once and an executable's entry point is not reachable from a test
/// (`docs/testing.md`, thin shell).
@MainActor
public final class MusterWindow: NSObject {
  public let window: NSWindow
  private let renderer: Renderer
  private let executable: String
  private let strip = RegionStrip(frame: NSRect(x: 0, y: 0, width: 960, height: 600))
  private let sidebar = SidebarView(frame: .zero)
  private let shortcuts = ShortcutsPanel()
  private let split = WindowLayout(frame: NSRect(x: 0, y: 0, width: 960, height: 600))
  private let empty = EmptyWindowView(frame: .zero)
  private var regions: [String: RegionView] = [:]

  /// Whether this window was opened with no daemon behind it on purpose.
  ///
  /// The one empty window that is not worth explaining a way out of, because there is none:
  /// `--renderer-check` proves the renderer paints and nothing here can make it a pane.
  private var rendererCheck = false

  /// The font size in force, so a pane made later gets it too. The core owns the number; this
  /// is the last one it published.
  private var fontSizeOffset: Int32 = 0

  /// Everything the attached daemons hold, whether or not this window is showing it.
  ///
  /// Held here rather than only in the sidebar because it is half of what the sidebar draws
  /// and the other half arrives separately: a roster and an agent state land in either
  /// order, and whichever is second has to be able to redraw with both.
  private var roster = Roster(daemons: [])

  /// Every pane's last known agent state, whether or not it is on screen.
  ///
  /// Held here rather than only in the chrome because the two arrive in either order: a pane
  /// created by a split is described by the daemon before its surface exists here, and a
  /// chrome built afterwards would otherwise render as `unknown` until that agent next moved.
  ///
  /// Keyed by daemon as well as pane, because this map spans regions and two daemons hand out
  /// the same pane ids - one keyed by pane alone would let a devenv agent paint a border on
  /// the laptop.
  private var states: [PaneKey: String] = [:]

  /// What each daemon last said about itself.
  ///
  /// Per daemon, because health is per connection: a devenv behind a dropped VPN says nothing
  /// about the laptop beside it, and one window-wide state would report the loss of both.
  private var health: [String: DaemonHealth] = [:]

  private struct DaemonHealth {
    let state: String
    let detail: String
  }

  private var keyboardPane: String?

  /// The pane the keyboard feeds, with the daemon that hands out its id.
  ///
  /// Held beside `keyboardPane` because the list spans daemons and a bare pane id does not:
  /// two daemons hand out the same ones, so a sidebar keyed on the id alone would light up a
  /// devenv row for a laptop pane.
  private var keyboardKey: PaneKey?

  /// Which region the keyboard is in, kept so that a chord can reach the surface it happened
  /// on. Held beside `keyboardKey` rather than searched for, because the pane id in that key
  /// is only unique within its own daemon.
  private var focusedRegionID: String?
  private var zoomed = false
  private var problem: String?

  /// The surface the keyboard is on, or nil when nothing is rendering yet.
  ///
  /// Only a live surface can say how big a cell is, and only the pane a chord happened on has
  /// the right answer: the font size is one window-wide setting, but a pane whose surface has
  /// not been sized yet has no measurement at all.
  private var keyboardSurface: SurfaceView? {
    guard let region = focusedRegionID, let pane = keyboardKey?.pane else { return nil }
    return regions[region]?.chrome(for: pane)?.surface
  }

  public init(renderer: Renderer, executable: String) {
    self.renderer = renderer
    self.executable = executable
    window = NSWindow(
      contentRect: strip.frame,
      styleMask: [.titled, .closable, .resizable, .miniaturizable],
      backing: .buffered,
      defer: false)
    super.init()
    split.attach(sidebar: sidebar, strip: strip)
    strip.attach(empty: empty)
    empty.apply(EmptyWindow.message(bindings: Core.bindings()))
    window.contentView = split
    window.delegate = self
    window.center()
    sidebar.onPanePicked = { pane in
      Core.focus(daemonID: pane.daemon, paneID: pane.pane)
    }
    sidebar.onTabPicked = { tab in
      Core.focus(tab: tab)
    }
    sidebar.onPaneArranged = { pane, onto in
      Core.arrange(pane: pane, onto: onto)
    }
    // Both halves name their subject outright rather than leaving it to whatever has the
    // keyboard: the row somebody double-clicked is very often a pane no region is showing,
    // which is what the list is for.
    sidebar.onRowRenamed = { [weak self] row in
      guard let self else { return }
      if let pane = row.pane {
        askToName(subject: "pane", current: row.givenName) { name in
          Core.renamePane(name: name, daemonID: pane.daemon, paneID: pane.pane)
        }
      } else if let tab = row.tab {
        askToName(subject: "tab", current: row.givenName) { name in
          Core.renameTab(name: name, daemonID: tab.daemon, tabID: tab.tab)
        }
      }
    }
    applyTitle()
  }

  public func show() {
    window.makeKeyAndOrderFront(nil)
  }

  /// Renders what the core says this window is showing.
  ///
  /// Whole-view and idempotent, so applying the same one twice is applying it once. Regions
  /// that survive keep their surfaces; the rest go, and their bridges go with them.
  public func apply(_ contents: WindowContents) {
    var order: [(id: String, weight: CGFloat, view: NSView)] = []
    for described in contents.regions {
      let region = regions[described.id] ?? make(regionID: described.id)
      order.append((id: described.id, weight: described.weight, view: region))
      region.apply(described, focused: described.id == contents.focusedRegion)
    }

    let named = Set(contents.regions.map(\.id))
    for (id, gone) in regions where !named.contains(id) {
      gone.removeFromSuperview()
      regions.removeValue(forKey: id)
    }
    strip.arrange(order)

    keyboardPane = contents.keyboardPane
    let focused = contents.regions.first { $0.id == contents.focusedRegion }
    focusedRegionID = focused?.id
    keyboardKey = focused.flatMap { region in
      region.keyboardPane.map { PaneKey(daemon: region.daemon, pane: $0) }
    }
    sidebar.apply(roster: roster, states: states, keyboard: keyboardKey)
    zoomed = focused?.zoomed ?? false
    empty.apply(showing: !contents.regions.isEmpty)
    applyTitle()
  }

  public func apply(pane: PaneKey, state: String) {
    states[pane] = state
    for region in regions.values where region.daemonID == pane.daemon {
      region.chrome(for: pane.pane)?.apply(paneID: pane.pane, state: state)
    }
    sidebar.apply(roster: roster, states: states, keyboard: keyboardKey)
  }

  /// Everything the daemons hold, whether or not this window is showing it.
  ///
  /// The list is the half of the founding desideratum the window cannot carry on its own: a
  /// pane no region shows has no border to colour, and it is the one most likely to have
  /// finished while nobody was looking.
  public func apply(_ roster: Roster) {
    self.roster = roster
    sidebar.apply(roster: roster, states: states, keyboard: keyboardKey)
  }

  /// What the window should be showing of itself, as the core decided it.
  ///
  /// Applied rather than toggled: the core holds the answer and sends it whole, including
  /// once at startup, so this window never has a default of its own to disagree with.
  public func apply(presentation: Presentation) {
    split.sidebarShown = presentation.sidebar
    // Held as well as applied, because a pane made after this arrives has to be sized too - a
    // split that opened at the configured size beside four panes somebody had made bigger is
    // the ragged grid this action exists to avoid.
    fontSizeOffset = presentation.fontSizeOffset
    for region in regions.values {
      for paneID in region.paneIDs {
        report(region.chrome(for: paneID)?.surface.setFontSizeOffset(fontSizeOffset) ?? [])
      }
    }
  }

  /// Says so when the renderer would not size a pane's text.
  ///
  /// Nothing in the suite can catch this: the actions are named by string, and validating one
  /// needs a live surface. So a version bump that renamed them shows up here rather than as a
  /// chord that quietly does nothing.
  private func report(_ refused: [String]) {
    guard !refused.isEmpty else { return }
    Core.warn(
      "renderer.action.refused",
      [
        "actions": refused.joined(separator: ", "),
        "impact": "the text in this pane is whatever size it already was, and the chord that "
          + "asked will keep doing nothing",
        "check": "whether libghostty renamed these between deps/ghostty.pin bumps; Muster "
          + "names them as strings and nothing else can tell",
      ])
  }

  /// Repaints the window after the config file was read again.
  ///
  /// The panes are libghostty's to repaint and the dividers are Muster's, which is the same
  /// split the launch path makes - the core sends one answer and the shell hands each half to
  /// whoever draws it.
  public func apply(appearance: Core.Appearance) {
    renderer.apply(appearance: appearance.pane)
    DividerView.repaint(with: appearance.dividerColor)
    for divider in dividers() {
      divider.recolor()
    }
  }

  /// Rebuilds the menu after the config file was read again.
  ///
  /// On macOS this is the whole of rebinding: a key equivalent on a menu item is where the
  /// platform dispatches a chord from, so a menu that still carries the old ones is a config
  /// that did not reload.
  public func apply(bindings: [Core.Binding]) {
    NSApp.mainMenu = AppMenu.build(target: self, bindings: bindings)
    // The empty window names a chord, so a rebind has to reach it too. A window sitting empty
    // while somebody edits the config file is exactly when a stale hint would be read.
    empty.apply(EmptyWindow.message(bindings: bindings))
  }

  /// Every divider on screen, region boundaries and pane splits alike.
  private func dividers() -> [DividerView] {
    var found: [DividerView] = []
    var pending: [NSView] = [split]
    while let view = pending.popLast() {
      if let divider = view as? DividerView { found.append(divider) }
      pending.append(contentsOf: view.subviews)
    }
    return found
  }

  public func apply(daemon: String, health state: String, detail: String) {
    health[daemon] = DaemonHealth(state: state, detail: detail)
    applyTitle()
  }

  /// A window with no daemon behind it: one surface running the user's shell.
  ///
  /// Not a degenerate case of the above, and deliberately not made to look like one. There is
  /// no pane, no core-published view and no control stream to put keystrokes on, so this
  /// renders and swallows the keyboard - which the title says, because a terminal that
  /// ignores you and does not explain itself is the worst thing this app could ship.
  public func showRendererCheck() {
    rendererCheck = true
    let region = make(regionID: "renderer-check")
    strip.arrange([(id: "renderer-check", weight: 1, view: region)])
    let chrome = PaneChrome(frame: strip.bounds, surface: SurfaceView(frame: strip.bounds))
    region.addSubview(chrome)
    chrome.autoresizingMask = [.width, .height]
    region.layoutSubtreeIfNeeded()
    start(chrome, command: ProcessInfo.processInfo.environment["SHELL"], typeable: false)
    applyTitle()
  }

  private func make(regionID: String) -> RegionView {
    let region = RegionView(frame: strip.bounds) {
      [weak self] daemonID, transport, herdrSocket, chrome, socketPath in
      self?.start(
        chrome, daemonID: daemonID, transport: transport, herdrSocket: herdrSocket,
        socketPath: socketPath)
    }
    strip.addSubview(region)
    regions[regionID] = region
    return region
  }

  /// Gives a pane's chrome a surface, and starts the bridge that paints it.
  private func start(
    _ chrome: PaneChrome, daemonID: String,
    transport: WindowContents.Region.Transport?, herdrSocket: String?, socketPath: String?
  ) {
    guard let paneID = chrome.paneID else { return }
    if let state = states[PaneKey(daemon: daemonID, pane: paneID)] {
      chrome.apply(paneID: paneID, state: state)
    }
    guard let socketPath else {
      // No channel is open for this pane yet, and a bridge started against nothing would
      // paint and then swallow every keystroke. The core opens one and republishes, and the
      // surface is built on that pass instead.
      Core.warn(
        "pane.surface.deferred",
        [
          "daemon": daemonID,
          "pane": paneID,
          "impact": "this pane is blank until the core opens its channel and republishes",
          "check": "a pane.channel.unavailable record above this, which says why one could "
            + "not be opened",
        ])
      return
    }
    // Reported rather than acted on here: the core is what can find out whether a bridge
    // ending means the daemon has dropped the pane, which is the commonest reason and the
    // one herdr does not always announce.
    chrome.surface.onProcessExited = { processAlive in
      Core.bridgeExited(daemonID: daemonID, paneID: paneID, processAlive: processAlive)
    }
    start(
      chrome,
      command: PaneCommand.bridge(
        executable: executable, paneID: paneID, controlSocketPath: socketPath,
        herdrSocketPath: herdrSocket,
        sshHost: transport?.sshHost, sshControlPath: transport?.sshControlPath),
      typeable: true)
  }

  private func start(_ chrome: PaneChrome, command: String?, typeable: Bool) {
    Core.info(
      "surface.create",
      ["pane": chrome.paneID ?? "(none)", "command": command ?? "(none)"])
    do {
      chrome.surface.attach(
        try renderer.makeSurface(in: chrome.surface, command: command), typeable: typeable)
      report(chrome.surface.setFontSizeOffset(fontSizeOffset))
    } catch {
      // One pane, not the window: the rest keep rendering, and a bug report needs to say
      // which one went missing rather than that something failed.
      Core.error(
        "pane.surface.failed",
        [
          "pane": chrome.paneID ?? "(none)",
          "error": "\(error)",
          "impact": "this pane renders nothing; every other pane in the window is unaffected",
        ])
    }
  }

  /// Says why this window is showing nothing, when it was asked to show something.
  ///
  /// The core has already explained itself on stderr and in the log. This is the part a user
  /// who is looking at the window can see, and without it an empty window titled as the
  /// renderer check reads as though the pane they named was fine.
  public func report(problem: String) {
    self.problem = problem
    applyTitle()
  }

  /// The daemon whose health the title should report, which is the unhappiest one.
  ///
  /// A window showing two daemons has two answers and one title bar. Reporting the worst
  /// names a session that is not being kept up to date; reporting anything else would let a
  /// stale devenv sit behind a title that says everything is fine, which is the failure the
  /// health state exists to prevent.
  ///
  /// Nothing attached is disconnected rather than fine, because a window with no daemon
  /// behind it is not a healthy window.
  private var worstHealth: (daemon: String, state: String, detail: String) {
    let ranked = ["connected": 0, "": 0, "stale": 1, "disconnected": 2]
    guard
      let worst = health.max(by: { (ranked[$0.value.state] ?? 3) < (ranked[$1.value.state] ?? 3) })
    else {
      return ("", "disconnected", "")
    }
    return (worst.key, worst.value.state, worst.value.detail)
  }

  private func applyTitle() {
    let (daemon, state, detail) = worstHealth
    window.title = PaneAppearance.title(
      paneID: keyboardPane, zoomed: zoomed, health: state, detail: detail, daemon: daemon,
      problem: problem, rendererCheck: rendererCheck)
  }
}

/// Whether anybody is looking at this window.
///
/// The only input to agent state that no daemon can supply. herdr derives `done` from the
/// foreground client's window focus and has no API to be told it, so a window sitting behind
/// a browser while an agent finishes is reported as `idle` - "nothing needs you", at the one
/// moment something does. The core decides what this means; this only says it happened.
///
/// Key window rather than app activation, because the question is whether this window was
/// being looked at. An app can be frontmost with this window behind its own settings sheet,
/// and the agent that finished underneath was no more seen than if the app were hidden.
extension MusterWindow: NSWindowDelegate {
  public func windowDidBecomeKey(_ notification: Notification) {
    Core.windowFocused(true)
  }

  public func windowDidResignKey(_ notification: Notification) {
    Core.windowFocused(false)
  }
}

// What the menu's items do. Every one of them is a request to the core and nothing else:
// the shell asks, the daemon answers, and the window changes when the view that comes back
// says it did (architecture.md, one action path).
extension MusterWindow {
  @objc public func splitRight(_ sender: Any?) {
    Core.split(side: "right")
  }

  @objc public func splitDown(_ sender: Any?) {
    Core.split(side: "down")
  }

  @objc public func splitLeft(_ sender: Any?) {
    Core.split(side: "left")
  }

  @objc public func splitUp(_ sender: Any?) {
    Core.split(side: "up")
  }

  @objc public func newTab(_ sender: Any?) {
    Core.createTab()
  }

  /// Names the pane the keyboard is on, having asked what to call it.
  ///
  /// The core decides which pane that is, as it does for every other item here - the roster is
  /// read only to start the field off with the name this pane already has, and a window whose
  /// roster has not caught up asks for a name from empty rather than refusing.
  @objc public func renamePane(_ sender: Any?) {
    askToName(subject: "pane", current: namedPane(keyboardKey)?.givenName ?? "") { name in
      Core.renamePane(name: name)
    }
  }

  /// Names the tab the keyboard's pane is in, having asked what to call it.
  @objc public func renameTab(_ sender: Any?) {
    askToName(subject: "tab", current: tabHolding(keyboardKey)?.givenName ?? "") { name in
      Core.renameTab(name: name)
    }
  }

  /// Runs the sheet, and sends what came back.
  ///
  /// Shared by the menu items and by a double-click in the list, so that all three are one
  /// path rather than three that drift. Nothing is drawn optimistically: the request goes out
  /// and the name appears when the daemon's answer comes back as the next roster.
  func askToName(subject: String, current: String, then send: @escaping (String) -> Void) {
    RenameSheet.ask(on: window, subject: subject, current: current, then: send)
  }

  /// This window's roster entry for a pane, if it has one.
  func namedPane(_ key: PaneKey?) -> Roster.Pane? {
    guard let key else { return nil }
    return roster.panes.first { $0.key == key }
  }

  /// The tab holding a pane, as the roster listed it.
  func tabHolding(_ key: PaneKey?) -> Roster.Tab? {
    guard let key else { return nil }
    return roster.tabs.first { tab in tab.panes.contains { $0.key == key } }
  }

  @objc public func closePane(_ sender: Any?) {
    Core.closePane()
  }

  // Each of the four reports the cell it is resizing by, because `resize_step` may be written
  // in points and only a live surface can say how many points a cell is.
  @objc public func resizePaneLeft(_ sender: Any?) {
    Core.resize(direction: "left", cell: keyboardSurface?.cellPointSize)
  }

  @objc public func resizePaneRight(_ sender: Any?) {
    Core.resize(direction: "right", cell: keyboardSurface?.cellPointSize)
  }

  @objc public func resizePaneUp(_ sender: Any?) {
    Core.resize(direction: "up", cell: keyboardSurface?.cellPointSize)
  }

  @objc public func resizePaneDown(_ sender: Any?) {
    Core.resize(direction: "down", cell: keyboardSurface?.cellPointSize)
  }

  @objc public func zoomPane(_ sender: Any?) {
    Core.zoom()
  }

  @objc public func increaseFontSize(_ sender: Any?) {
    Core.adjustFontSize("larger")
  }

  @objc public func decreaseFontSize(_ sender: Any?) {
    Core.adjustFontSize("smaller")
  }

  @objc public func resetFontSize(_ sender: Any?) {
    Core.adjustFontSize("reset")
  }

  @objc public func reloadConfig(_ sender: Any?) {
    Core.reloadConfig()
  }

  @objc public func toggleSidebar(_ sender: Any?) {
    Core.toggleSidebar()
  }

  /// Opens the list of what this window does. Answered here rather than by the core: the
  /// list is built from what the core already publishes, and a window is a shell's to open.
  @objc public func showShortcuts(_ sender: Any?) {
    shortcuts.show(bindings: Core.bindings())
  }

  @objc public func focusNextTab(_ sender: Any?) {
    Core.focus(tabStep: "next")
  }

  @objc public func focusPreviousTab(_ sender: Any?) {
    Core.focus(tabStep: "previous")
  }

  /// Goes to the pane this item was numbered for.
  ///
  /// One method for all nine, reading the place off the item's tag. Nine methods differing by
  /// a digit is nine places for one of them to drift, and the number is data anyway.
  @objc public func focusPaneAtPlace(_ sender: Any?) {
    guard let item = sender as? NSMenuItem, item.tag > 0 else { return }
    Core.focus(panePlace: item.tag)
  }

  @objc public func focusNextPane(_ sender: Any?) {
    Core.focus(step: "next")
  }

  @objc public func focusPreviousPane(_ sender: Any?) {
    Core.focus(step: "previous")
  }

  @objc public func focusPaneLeft(_ sender: Any?) {
    Core.focus(step: "left")
  }

  @objc public func focusPaneRight(_ sender: Any?) {
    Core.focus(step: "right")
  }

  @objc public func focusPaneUp(_ sender: Any?) {
    Core.focus(step: "up")
  }

  @objc public func focusPaneDown(_ sender: Any?) {
    Core.focus(step: "down")
  }
}

/// The sidebar down the left, and everything else to the right of it.
///
/// Its own view rather than arithmetic inside the window, so that the one number here - how
/// much width the list takes - is a pure function a test can call. The list is a fixed width
/// because it holds a directory and a harness name and nothing that benefits from more; the
/// regions get what is left.
@MainActor
final class WindowLayout: NSView {
  private var sidebar: NSView?
  private var strip: NSView?

  /// Whether the core says the list belongs on screen. Mirrored, never decided: the answer
  /// is written down beside the arrangement and comes back on the next launch.
  var sidebarShown = true {
    didSet { needsLayout = true }
  }

  override var isFlipped: Bool { true }

  func attach(sidebar: NSView, strip: NSView) {
    self.sidebar = sidebar
    self.strip = strip
    addSubview(sidebar)
    addSubview(strip)
    needsLayout = true
  }

  override func layout() {
    super.layout()
    let (listWidth, regionWidth) = SidebarModel.widths(in: bounds.width, shown: sidebarShown)
    sidebar?.frame = CGRect(x: 0, y: 0, width: listWidth, height: bounds.height)
    sidebar?.isHidden = listWidth == 0
    strip?.frame = CGRect(x: listWidth, y: 0, width: regionWidth, height: bounds.height)
    strip?.needsLayout = true
  }
}

/// Where each region sits, and where the lines between them are.
///
/// A pure function, in the same spirit as `PaneTree.frames`: this is the arithmetic, and
/// arithmetic inside `layout` is arithmetic no test can call. A wrong frame here looks like a
/// rendering problem and is a division.
///
/// Weights rather than a tree, because Muster owns no tree over regions - owning one is what
/// would make it a multiplexer, a non-goal - so the whole arrangement is a list and a
/// division by the sum of it.
enum RegionStripLayout {
  struct Placement: Equatable {
    let frame: CGRect
    /// The line on this region's right, and the area it shares with its neighbour. Absent on
    /// the last region, which has nothing to its right to divide against.
    let divider: CGRect?
    let area: CGRect?
  }

  /// The same thickness a pane divider has. Two lines of different weights in one window
  /// would read as two different kinds of thing, and they are the same thing one level apart.
  static let dividerThickness = PaneTree.dividerThickness

  static func place(weights: [CGFloat], in bounds: CGRect) -> [Placement] {
    guard !weights.isEmpty else { return [] }
    // A weight that is not a positive number is not this window's to validate - it came
    // across the seam - so it is read as an equal share rather than collapsing the region to
    // nothing or poisoning the total with a NaN.
    let sane = weights.map { $0.isFinite && $0 > 0 ? $0 : 1 }
    let total = sane.reduce(0, +)
    let lines = CGFloat(weights.count - 1) * dividerThickness
    let usable = max(0, bounds.width - lines)

    var placements: [Placement] = []
    var x = bounds.minX
    for (index, weight) in sane.enumerated() {
      let width = usable * weight / total
      let frame = CGRect(x: x, y: bounds.minY, width: width, height: bounds.height)
      x += width
      guard index < sane.count - 1 else {
        placements.append(Placement(frame: frame, divider: nil, area: nil))
        continue
      }
      let divider = CGRect(
        x: x, y: bounds.minY, width: dividerThickness, height: bounds.height)
      // What the pair shares, which is what a drag's ratio is measured against - the two
      // regions plus the line between them, and nothing further along the window.
      let next = usable * sane[index + 1] / total
      let area = CGRect(
        x: frame.minX, y: bounds.minY, width: width + dividerThickness + next,
        height: bounds.height)
      x += dividerThickness
      placements.append(Placement(frame: frame, divider: divider, area: area))
    }
    return placements
  }
}

/// Regions, side by side, in the order the core listed them, at the widths it gave them.
///
/// Every region starts at the same weight, so equal shares are what a window that has never
/// been dragged looks like - the floor this used to hardcode, now falling out of the
/// arrangement rather than being it.
///
/// A drag moves nothing locally. It asks the core, which owns this arrangement outright, and
/// the strip lands where the next published view puts it - the same discipline as a pane
/// divider, for the same reason.
@MainActor
final class RegionStrip: NSView {
  private var order: [NSView] = []
  private var identities: [String] = []
  private var weights: [CGFloat] = []
  private var dividers: [DividerView] = []

  /// What fills the strip when no region does. Behind the regions rather than beside them,
  /// so it needs no share of the arrangement and a region drawn over it hides it.
  private var empty: NSView?

  override var isFlipped: Bool { true }

  func attach(empty: NSView) {
    self.empty = empty
    addSubview(empty, positioned: .below, relativeTo: nil)
    needsLayout = true
  }

  func arrange(_ regions: [(id: String, weight: CGFloat, view: NSView)]) {
    order = regions.map(\.view)
    identities = regions.map(\.id)
    weights = regions.map(\.weight)
    needsLayout = true
  }

  override func layout() {
    super.layout()
    empty?.frame = bounds
    guard !order.isEmpty else { return }
    let placements = RegionStripLayout.place(weights: weights, in: bounds)

    let wanted = placements.filter { $0.divider != nil }.count
    while dividers.count < wanted {
      let divider = DividerView(frame: .zero)
      addSubview(divider)
      dividers.append(divider)
    }
    while dividers.count > wanted {
      dividers.removeLast().removeFromSuperview()
    }

    var line = 0
    for (index, placement) in placements.enumerated() {
      order[index].frame = placement.frame
      order[index].needsLayout = true
      guard let frame = placement.divider, let area = placement.area else { continue }
      let divider = dividers[line]
      line += 1
      let region = identities[index]
      divider.onDrag = { ratio in Core.setRegionBoundary(region: region, ratio: ratio) }
      divider.axis = .columns
      divider.area = area
      divider.frame = frame
      // Cursor rectangles are cached against the frame they were set from, so a divider that
      // moved shows the wrong resize cursor - or none - until this is asked for.
      window?.invalidateCursorRects(for: divider)
    }
  }
}
