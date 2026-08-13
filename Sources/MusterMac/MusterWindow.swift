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
  private var regions: [String: RegionView] = [:]

  /// Every pane's last known agent state, whether or not it is on screen.
  ///
  /// Held here rather than only in the chrome because the two arrive in either order: a pane
  /// created by a split is described by the daemon before its surface exists here, and a
  /// chrome built afterwards would otherwise render as `unknown` until that agent next moved.
  private var states: [String: String] = [:]

  private var keyboardPane: String?
  private var zoomed = false
  private var health = "disconnected"
  private var detail = ""
  private var problem: String?

  public init(renderer: Renderer, executable: String) {
    self.renderer = renderer
    self.executable = executable
    window = NSWindow(
      contentRect: strip.frame,
      styleMask: [.titled, .closable, .resizable, .miniaturizable],
      backing: .buffered,
      defer: false)
    super.init()
    window.contentView = strip
    window.center()
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
    var order: [RegionView] = []
    for described in contents.regions {
      let region = regions[described.id] ?? make(regionID: described.id)
      order.append(region)
      region.apply(described, focused: described.id == contents.focusedRegion)
    }

    let named = Set(contents.regions.map(\.id))
    for (id, gone) in regions where !named.contains(id) {
      gone.removeFromSuperview()
      regions.removeValue(forKey: id)
    }
    strip.arrange(order)

    keyboardPane = contents.keyboardPane
    zoomed = contents.regions.first { $0.id == contents.focusedRegion }?.zoomed ?? false
    applyTitle()
  }

  public func apply(paneID: String, state: String) {
    states[paneID] = state
    for region in regions.values {
      region.chrome(for: paneID)?.apply(paneID: paneID, state: state)
    }
  }

  public func apply(health: String, detail: String) {
    self.health = health
    self.detail = detail
    applyTitle()
  }

  /// A window with no daemon behind it: one surface running the user's shell.
  ///
  /// Not a degenerate case of the above, and deliberately not made to look like one. There is
  /// no pane, no core-published view and no control stream to put keystrokes on, so this
  /// renders and swallows the keyboard - which the title says, because a terminal that
  /// ignores you and does not explain itself is the worst thing this app could ship.
  public func showRendererCheck() {
    let region = make(regionID: "renderer-check")
    strip.arrange([region])
    let chrome = PaneChrome(frame: strip.bounds, surface: SurfaceView(frame: strip.bounds))
    region.addSubview(chrome)
    chrome.autoresizingMask = [.width, .height]
    region.layoutSubtreeIfNeeded()
    start(chrome, command: ProcessInfo.processInfo.environment["SHELL"], typeable: false)
    applyTitle()
  }

  private func make(regionID: String) -> RegionView {
    let region = RegionView(frame: strip.bounds) { [weak self] chrome, socketPath in
      self?.start(chrome, socketPath: socketPath)
    }
    strip.addSubview(region)
    regions[regionID] = region
    return region
  }

  /// Gives a pane's chrome a surface, and starts the bridge that paints it.
  private func start(_ chrome: PaneChrome, socketPath: String?) {
    guard let paneID = chrome.paneID else { return }
    if let state = states[paneID] {
      chrome.apply(paneID: paneID, state: state)
    }
    guard let socketPath else {
      // No channel is open for this pane yet, and a bridge started against nothing would
      // paint and then swallow every keystroke. The core opens one and republishes, and the
      // surface is built on that pass instead.
      Core.warn(
        "pane.surface.deferred",
        [
          "pane": paneID,
          "impact": "this pane is blank until the core opens its channel and republishes",
          "check": "a pane.channel.unavailable record above this, which says why one could "
            + "not be opened",
        ])
      return
    }
    start(
      chrome,
      command: PaneCommand.bridge(
        executable: executable, paneID: paneID, controlSocketPath: socketPath),
      typeable: true)
  }

  private func start(_ chrome: PaneChrome, command: String?, typeable: Bool) {
    Core.info(
      "surface.create",
      ["pane": chrome.paneID ?? "(none)", "command": command ?? "(none)"])
    do {
      chrome.surface.attach(
        try renderer.makeSurface(in: chrome.surface, command: command), typeable: typeable)
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

  private func applyTitle() {
    window.title = PaneAppearance.title(
      paneID: keyboardPane, zoomed: zoomed, health: health, detail: detail, problem: problem)
  }
}

// What the menu's items do. Every one of them is a request to the core and nothing else:
// the shell asks, the daemon answers, and the window changes when the view that comes back
// says it did (architecture.md, one action path).
extension MusterWindow {
  @objc public func splitRight(_ sender: Any?) {
    Core.split(axis: SplitAxis.columns.rawValue)
  }

  @objc public func splitDown(_ sender: Any?) {
    Core.split(axis: SplitAxis.rows.rawValue)
  }

  @objc public func closePane(_ sender: Any?) {
    Core.closePane()
  }

  @objc public func focusNextPane(_ sender: Any?) {
    Core.focus(step: "next")
  }

  @objc public func focusPreviousPane(_ sender: Any?) {
    Core.focus(step: "previous")
  }
}

/// Regions, side by side, in the order the core listed them.
///
/// Equal widths, because Muster owns no tree over regions and has nothing else to divide them
/// by. That is a deliberate floor rather than the finished answer - dragging the line between
/// a laptop and a devenv is its own card - and equal shares are the arrangement nobody has to
/// be told about.
@MainActor
final class RegionStrip: NSView {
  private var order: [NSView] = []

  override var isFlipped: Bool { true }

  func arrange(_ regions: [NSView]) {
    order = regions
    needsLayout = true
  }

  override func layout() {
    super.layout()
    guard !order.isEmpty else { return }
    let width = bounds.width / CGFloat(order.count)
    for (index, region) in order.enumerated() {
      region.frame = CGRect(
        x: bounds.minX + width * CGFloat(index), y: bounds.minY,
        width: width, height: bounds.height)
      region.needsLayout = true
    }
  }
}
