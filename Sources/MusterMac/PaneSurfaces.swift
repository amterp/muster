import AppKit

/// Every pane's surface, for as long as its daemon holds the pane.
///
/// One surface per pane per window, which is a stronger rule than the one it replaces and
/// answers three separate failures with it.
///
/// **A surface used to belong to the region showing it**, so switching a region to another tab
/// destroyed the old tab's surfaces and the bridges they spawned, and switching back built
/// them again. On a devenv that costs about 440ms of ssh session setup per switch, because
/// every new bridge is a new `ssh` exec and the far machine has to spawn a session for it -
/// measured at 444-561ms against 29-59ms for a local pane. Held here instead, a pane pays it
/// once and switching is free (kan a_2HzbwzO32).
///
/// **Two regions showing one tab used to build two surfaces**, and only one bridge may draw a
/// pane - the other printed a refusal and could not be closed. The core no longer opens two
/// such regions, and a store keyed by pane cannot produce two surfaces even if it ever does
/// again (kan a_2Ht74jTXV).
///
/// **A pane's border and badge used to be found by walking the regions**, which reached only
/// what was on screen. Keyed by pane, a parked pane keeps its state painted, so a tab switched
/// back to is right on the first frame rather than on the next agent transition.
///
/// What it costs is what the card priced: one `muster-bridge` per pane, held for as long as the
/// pane exists, rather than per pane on screen. They are released when the daemon stops holding
/// the pane, so the cost scales with panes rather than with switches.
@MainActor
public final class PaneSurfaces {
  /// Gives a pane's chrome a surface, and the bridge that feeds it.
  ///
  /// Injected because the real one needs a GPU, a libghostty runtime and a subprocess, and
  /// what this class decides - which panes have surfaces, which region is showing each, when
  /// one is let go - is worth testing without any of the three.
  public typealias StartPane =
    @MainActor (
      _ region: WindowContents.Region, _ chrome: PaneChrome, _ pane: PaneTree.Leaf
    ) -> Void

  private struct Held {
    let chrome: PaneChrome

    /// Where its bridge was to report when it was built. A pane whose socket changed needs a
    /// new bridge, and a bridge is spawned by its surface's command - so it needs a new
    /// surface too.
    let linkSocketPath: String?

    /// Where its bridge was to dial when it was built. Only its absence is acted on: a pane
    /// whose daemon had no socket yet was never given a bridge, and gets one once there is a
    /// socket to dial.
    let daemonSocket: String?

    /// Which replacement its bridge was on when it was built. The core counts these, and a
    /// number that has moved means the bridge behind this surface has ended - most often
    /// because the connection carrying it died - so the surface has to be built again to
    /// start another.
    let bridgeRestarts: UInt32
  }

  private let startPane: StartPane

  /// Where a chrome waits while no region is showing its pane.
  ///
  /// Hidden, and inside the window rather than held with no superview. A surface is handed to
  /// libghostty as a view, and keeping that view in a window for its whole life is the state
  /// this code has always run in - a parked chrome that had left the hierarchy would be a new
  /// one, and the difference is only visible on a GPU.
  private let parking = NSView(frame: .zero)

  private var held: [PaneKey: Held] = [:]
  /// The panes the last roster named, or nil before one has arrived.
  private var alive: Set<PaneKey>?
  private var windowIsVisible = true
  private var windowIsKey = false

  /// Builds a pane's context menu, handed to each surface with the pane it shows.
  public var menu: (@MainActor (PaneKey) -> NSMenu?)?

  public init(parkedIn container: NSView, startPane: @escaping StartPane) {
    self.startPane = startPane
    parking.isHidden = true
    container.addSubview(parking)
  }

  public func chrome(for key: PaneKey) -> PaneChrome? { held[key]?.chrome }

  /// Tells every surface whether the window can be seen, and every one made later too. A hidden
  /// window's panes would otherwise draw at full rate for nobody: the app holds App Nap off, so
  /// nothing else slows them.
  public func window(visible: Bool) {
    windowIsVisible = visible
    for chrome in chromes { chrome.surface.apply(windowIsVisible: visible) }
  }

  /// Tells every surface whether the window is key, so that only a key window's pane is focused.
  public func window(key: Bool) {
    windowIsKey = key
    for chrome in chromes { chrome.surface.apply(windowIsKey: key) }
  }

  /// Every pane this window is holding, on screen or parked. For a repaint that is about the
  /// colours rather than about any one pane - a config file saved changes no state, so nothing
  /// else would ever tell a parked pane its border moved.
  public var chromes: [PaneChrome] { held.values.map(\.chrome) }

  /// The chrome for a pane a region is about to show, and whether it needs a surface.
  ///
  /// The caller adds it to the region, lays the region out, and only then starts what this
  /// said was new - because libghostty is handed a view and sizes its surface from it, so a
  /// surface created against a zero-sized view is a PTY told it has no columns.
  ///
  /// A pane whose link socket moved is torn down and built again rather than reused. Left
  /// alone its bridge would keep reporting to a socket nothing is listening on, and the window
  /// would never hear that it died.
  public func borrow(
    daemonID: String, daemonSocket: String?, leaf: PaneTree.Leaf,
    focus: @escaping (String) -> Void, pointer: @escaping (String, Core.Pointer) -> Void
  ) -> (chrome: PaneChrome, isNew: Bool) {
    let key = PaneKey(daemon: daemonID, pane: leaf.paneID)
    if let existing = held[key] {
      let neverDialed = existing.daemonSocket == nil && daemonSocket != nil
      if existing.linkSocketPath == leaf.linkSocketPath,
        existing.bridgeRestarts == leaf.bridgeRestarts, !neverDialed
      {
        return (existing.chrome, false)
      }
      let reason =
        if existing.linkSocketPath != leaf.linkSocketPath {
          "its link socket changed, so its bridge was reporting to a closed listener"
        } else if neverDialed {
          "its daemon has a socket now, and it was never given a bridge without one"
        } else {
          "the core replaced its bridge, which only a new surface can start"
        }
      Core.info(
        "pane.surface.rebuilt",
        ["pane": leaf.paneID, "reason": reason, "bridge_restarts": String(leaf.bridgeRestarts)])
      release(key)
    }

    let chrome = PaneChrome(frame: .zero, surface: SurfaceView(frame: .zero))
    chrome.surface.apply(windowIsVisible: windowIsVisible)
    chrome.surface.apply(windowIsKey: windowIsKey)
    chrome.attach(paneID: leaf.paneID)
    chrome.onFocusRequested = focus
    chrome.onPointerRequested = pointer
    chrome.surface.onMenu = { [weak self] in self?.menu?(key) }
    chrome.key = key
    chrome.onPaneDropped = { dragged, side in Core.arrange(pane: dragged, onto: key, side: side) }
    held[key] = Held(
      chrome: chrome, linkSocketPath: leaf.linkSocketPath, daemonSocket: daemonSocket,
      bridgeRestarts: leaf.bridgeRestarts)
    return (chrome, true)
  }

  /// Starts the bridge for a pane that has just been given a chrome and laid out.
  public func start(in region: WindowContents.Region, chrome: PaneChrome, leaf: PaneTree.Leaf) {
    startPane(region, chrome, leaf)
  }

  /// Takes back every chrome no region is showing, and keeps it alive off screen.
  ///
  /// In one pass over the whole window rather than per region, and it has to be: a pane that
  /// moved from one region to another is claimed by the second and given up by the first, and
  /// a region that parked its own departures as it applied would take back a chrome the region
  /// beside it had already adopted - whenever the two applied in that order.
  ///
  /// A pane the last roster left out is let go as soon as it is parked, rather than waiting for
  /// the next roster. A window that skips a stale view applies that view's roster while the pane
  /// is still on screen, and the roster after it may be a long time coming.
  public func park(everythingBut onScreen: Set<PaneKey>) {
    for (key, entry) in held where !onScreen.contains(key) {
      guard entry.chrome.superview !== parking else { continue }
      parking.addSubview(entry.chrome)
    }
    if let alive { release(everythingBut: alive) }
  }

  /// Lets go of every parked pane the daemons no longer hold.
  ///
  /// Driven by the roster, which is the one message naming every pane on every attached
  /// daemon whether or not a region is showing it. A pane that closed while its tab was off
  /// screen leaves this way; without it, a window that visits fifteen tabs holds fifteen
  /// tabs' worth of bridges until it quits.
  ///
  /// Only parked panes, never one a region is showing, so a pane on screen survives a roster
  /// that does not name it. A parked pane has no such cover: a roster naming nothing releases
  /// every one of them. That is safe only because a roster cannot arrive empty while a daemon
  /// holds panes - a mirror is seeded from its snapshot before its backend is registered, and
  /// nothing empties one afterwards except the next snapshot. If that stops being true, this
  /// is where a window loses every off-screen bridge to a daemon that was only reconnecting.
  public func release(everythingBut alive: Set<PaneKey>) {
    self.alive = alive
    for (key, entry) in held
    where !alive.contains(key) && entry.chrome.superview === parking {
      release(key)
    }
  }

  /// Drops every pane's chrome, on screen or parked, when the window holding them closes.
  public func releaseAll() {
    for key in Array(held.keys) {
      release(key)
    }
  }

  /// How many panes have a surface, for a test to price what this holds.
  public var count: Int { held.count }

  /// Which panes are parked, for a test to say what is being held off screen.
  public var parked: Set<PaneKey> {
    Set(held.filter { $0.value.chrome.superview === parking }.keys)
  }

  /// Drops one pane's chrome, which drops its surface, which ends the bridge that surface
  /// spawned - so the pane's bridge exits here rather than being left dialing a window that
  /// has forgotten it.
  private func release(_ key: PaneKey) {
    held.removeValue(forKey: key)?.chrome.removeFromSuperview()
  }
}
