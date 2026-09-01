import AppKit

/// One region of the window: one tab's pane tree, rendered.
///
/// Rebuilt from the whole view every time, and yet it tears nothing down that survives.
/// Those are not in tension - the core publishes the whole arrangement because that is what
/// keeps the shell from holding a picture it has to patch, and this diffs against what it
/// already has because a surface is expensive and a rebuilt surface is a visible flicker.
/// The rule is that identity is the pane id: same id, same surface, whatever moved around it
/// (`architecture.md`, rendering is driven by diffs scoped to what changed).
///
/// Identity is settled when a view arrives; geometry is settled in `layout`. Keeping them
/// apart is what makes a window resize cost arithmetic and no surfaces.
@MainActor
public final class RegionView: NSView {
  /// Where the panes come from. Borrowed for as long as this region shows them and handed
  /// back when it stops, because a surface outlives the region that was showing it: switching
  /// a tab away and back would otherwise cost a new bridge and, on a devenv, the 440ms its
  /// machine takes to open a session for one.
  private let surfaces: PaneSurfaces
  private var showing: [String] = []
  private var dividers: [DividerView] = []
  private var tree: PaneTree?

  public private(set) var regionID: String = ""

  /// Which daemon this region's tab lives on.
  ///
  /// Held rather than passed around because every intent this region raises names it, and
  /// because the window above keys agent state by it: pane ids repeat across daemons, so a
  /// click or a state that travelled without this would land on whichever `w1:p1` was found
  /// first.
  public private(set) var daemonID: String = ""

  /// How this region's panes are reached, when they are on another machine.
  public private(set) var transport: WindowContents.Region.Transport?

  /// Which daemon this region's frames come from, when it is on this machine.
  public private(set) var backendSocket: String?
  private var tab: String = ""

  /// Carries divider positions to the core without stalling the drag. One per region rather
  /// than one per divider, because a person drags one line at a time and a sender per pooled
  /// view would let two of them be in flight at once.
  ///
  /// Reachable so a test can wait for the round trip it started; the app never looks at it.
  public let dividerPositions = SplitRatioSender()

  public init(frame: NSRect, surfaces: PaneSurfaces) {
    self.surfaces = surfaces
    super.init(frame: frame)
    wantsLayer = true
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  /// Top-left origin, so that "the first child is the upper one" is what the arithmetic says.
  /// A region laid out bottom-up would have to invert every row split on the way out, which
  /// is one inversion away from a window that renders every stack upside down.
  public override var isFlipped: Bool { true }

  /// The panes this region is showing, in reading order.
  public var paneIDs: [String] { tree?.leaves.map(\.paneID) ?? [] }

  /// The panes with a surface in this region right now, which is not always the same list.
  /// A tree that named a pane the daemon had not described yet gets no chrome until it does.
  public var onScreen: Set<PaneKey> {
    Set(showing.map { PaneKey(daemon: daemonID, pane: $0) })
  }

  public func chrome(for paneID: String) -> PaneChrome? {
    guard showing.contains(paneID) else { return nil }
    return surfaces.chrome(for: PaneKey(daemon: daemonID, pane: paneID))
  }

  /// Renders a region, keeping every surface the new tree still names.
  ///
  /// A nil tree leaves what is on screen alone. The daemon has not said how this tab is
  /// arranged - it publishes the tree on its own event, which may follow the panes it names -
  /// and tearing surfaces down for a moment that resolves in milliseconds is a flicker on
  /// every split.
  ///
  /// Nothing is let go here. The window parks whatever the whole arrangement stops showing,
  /// in one pass before any region applies, because a pane that moved between two regions is
  /// named by one of them and neither region can see the other's answer.
  public func apply(_ region: WindowContents.Region, focused: Bool) {
    regionID = region.id
    daemonID = region.daemon
    transport = region.transport
    backendSocket = region.backendSocket
    tab = region.tab
    guard let tree = region.tree else { return }
    self.tree = tree
    let fresh = takePanes(tree.leaves)
    // Laid out before the surfaces are made, so each one is handed a view that already has
    // the size it will keep.
    needsLayout = true
    layoutSubtreeIfNeeded()
    for leaf in fresh {
      guard let chrome = chrome(for: leaf.paneID) else { continue }
      surfaces.start(
        daemonID: daemonID, transport: transport, backendSocket: backendSocket, chrome: chrome,
        leaf: leaf)
    }
    apply(keyboardPane: focused ? region.keyboardPane : nil)
  }

  /// Puts this region's panes in it, and says which of them still need a surface.
  ///
  /// A pane already here is left where it is: `addSubview` on a view that has this superview
  /// already still reorders it, and a surface reordered every publish is a flicker on every
  /// agent transition.
  private func takePanes(_ leaves: [PaneTree.Leaf]) -> [PaneTree.Leaf] {
    var fresh: [PaneTree.Leaf] = []
    showing = leaves.map(\.paneID)
    for leaf in leaves {
      let daemon = daemonID
      let taken = surfaces.borrow(
        daemonID: daemon, leaf: leaf,
        focus: { paneID in Core.focus(daemonID: daemon, paneID: paneID) },
        scroll: { paneID, direction, delta in
          Core.scroll(daemonID: daemon, paneID: paneID, direction: direction, delta: delta)
        })
      if taken.chrome.superview !== self {
        taken.chrome.frame = bounds
        addSubview(taken.chrome)
      }
      if taken.isNew { fresh.append(leaf) }
    }
    return fresh
  }

  /// Points the keyboard at a pane, and says so on screen.
  ///
  /// The responder move is what makes libghostty draw a live cursor in one pane and a hollow
  /// one in the rest, which is the platform's own answer to "which of these am I typing
  /// into". It follows the core's view rather than leading it: AppKit's own focus is a
  /// consequence here, never the source.
  private func apply(keyboardPane: String?) {
    for paneID in showing {
      chrome(for: paneID)?.apply(focused: paneID == keyboardPane)
    }
    guard let keyboardPane, let chrome = chrome(for: keyboardPane), let window else { return }
    if window.firstResponder !== chrome.surface {
      window.makeFirstResponder(chrome.surface)
    }
  }

  public override func layout() {
    super.layout()
    guard let tree else { return }
    let frames = tree.frames(in: bounds)

    while dividers.count < frames.dividers.count {
      let divider = DividerView(frame: .zero)
      addSubview(divider)
      dividers.append(divider)
    }
    while dividers.count > frames.dividers.count {
      dividers.removeLast().removeFromSuperview()
    }

    for placement in frames.panes {
      guard let chrome = chrome(for: placement.paneID) else { continue }
      chrome.frame = placement.frame
      chrome.needsLayout = true
    }
    for (divider, placement) in zip(dividers, frames.dividers) {
      // Rebound every pass rather than once at pooling, because what a divider is named by is a
      // property of where it landed this time - the pool holds views and nothing else.
      let path = placement.path
      divider.onDrag = { [weak self] ratio in
        guard let self else { return }
        self.dividerPositions.send(
          daemonID: self.daemonID, tab: self.tab, path: path, ratio: ratio)
      }
      divider.axis = placement.axis
      divider.area = placement.area
      divider.frame = placement.frame
      // Cursor rectangles are cached against the frame they were set from, so a divider that
      // moved shows the wrong resize cursor - or none - until this is asked for.
      window?.invalidateCursorRects(for: divider)
    }
  }
}
