import AppKit

/// The tab on screen: its one tree over every machine it spans, rendered.
///
/// Rebuilt from the whole view every time, and yet it tears nothing down that survives.
/// Those are not in tension - the core publishes the whole arrangement because that is what
/// keeps the shell from holding a picture it has to patch, and this diffs against what it
/// already has because a surface is expensive and a rebuilt surface is a visible flicker.
/// The rule is that identity is the pane: same pane, same surface, whatever moved around it
/// (`architecture.md`, rendering is driven by diffs scoped to what changed).
///
/// Identity is settled when a view arrives; geometry is settled in `layout`. Keeping them
/// apart is what makes a window resize cost arithmetic and no surfaces.
@MainActor
public final class TabView: NSView {
  /// Where the panes come from. Borrowed for as long as this tab shows them and handed back
  /// when it stops, because a surface outlives the tab that was showing it: switching a tab
  /// away and back would otherwise cost a new bridge and, on a devenv, the 440ms its machine
  /// takes to open a session for one.
  private let surfaces: PaneSurfaces
  private var showing: [PaneKey] = []
  private var dividers: [DividerView] = []
  private var tree: PaneTree?
  private var tab: String?

  /// What fills the tab when it has no panes. Behind them rather than beside them, so a pane
  /// drawn over it hides it.
  private var empty: NSView?

  /// Carries divider positions to the core without stalling the drag. One for the tab rather
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
  /// A tab laid out bottom-up would have to invert every row split on the way out, which is
  /// one inversion away from a window that renders every stack upside down.
  public override var isFlipped: Bool { true }

  func attach(empty: NSView) {
    self.empty = empty
    addSubview(empty, positioned: .below, relativeTo: nil)
    needsLayout = true
  }

  /// The panes this tab is showing, in reading order.
  public var panes: [PaneKey] { tree?.leaves.map(\.key) ?? [] }

  /// The panes with a surface here right now, which is not always the same list. A tree that
  /// named a pane the daemon had not described yet gets no chrome until it does.
  public var onScreen: Set<PaneKey> { Set(showing) }

  public func chrome(for key: PaneKey) -> PaneChrome? {
    guard showing.contains(key) else { return nil }
    return surfaces.chrome(for: key)
  }

  /// Renders the tab, keeping every surface the new tree still names.
  ///
  /// A nil tree for the tab already here leaves what is on screen alone. No machine has said
  /// how the tab is arranged yet - a daemon publishes the tree on its own event, which may
  /// follow the panes it names - and tearing surfaces down for a moment that resolves in
  /// milliseconds is a flicker on every split. A nil tree for another tab shows nothing, since
  /// what is here belongs to the tab that went.
  ///
  /// Nothing is let go here. The window parks whatever the view stops showing before this
  /// applies, so a pane that moved is named once.
  public func apply(_ contents: WindowContents) {
    let sameTab = contents.tab == tab
    tab = contents.tab
    guard let tree = contents.tree else {
      if !sameTab {
        self.tree = nil
        showing = []
        needsLayout = true
      }
      return
    }
    self.tree = tree
    let fresh = takePanes(tree.leaves, machines: contents.machines)
    // Laid out before the surfaces are made, so each one is handed a view that already has
    // the size it will keep.
    needsLayout = true
    layoutSubtreeIfNeeded()
    for leaf in fresh {
      guard let chrome = chrome(for: leaf.key) else { continue }
      surfaces.start(on: contents.machines[leaf.daemon], chrome: chrome, leaf: leaf)
    }
    apply(keyboard: contents.keyboard)
  }

  /// Puts the tab's panes in it, and says which of them still need a surface.
  ///
  /// A pane already here is left where it is: `addSubview` on a view that has this superview
  /// already still reorders it, and a surface reordered every publish is a flicker on every
  /// agent transition.
  private func takePanes(
    _ leaves: [PaneTree.Leaf], machines: [String: WindowContents.Machine]
  ) -> [PaneTree.Leaf] {
    var fresh: [PaneTree.Leaf] = []
    showing = leaves.map(\.key)
    for leaf in leaves {
      let daemon = leaf.daemon
      let taken = surfaces.borrow(
        daemonID: daemon, daemonSocket: machines[daemon]?.socket, leaf: leaf,
        focus: { paneID in Core.focus(daemonID: daemon, paneID: paneID) },
        pointer: { paneID, pointer in Core.pointer(daemonID: daemon, paneID: paneID, pointer) })
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
  private func apply(keyboard: PaneKey?) {
    for key in showing {
      chrome(for: key)?.apply(focused: key == keyboard)
    }
    guard let keyboard, let chrome = chrome(for: keyboard), let window else { return }
    if window.firstResponder !== chrome.surface {
      window.makeFirstResponder(chrome.surface)
    }
  }

  public override func layout() {
    super.layout()
    empty?.frame = bounds
    guard let tree, let tab else {
      while let divider = dividers.popLast() { divider.removeFromSuperview() }
      return
    }
    let frames = tree.frames(in: bounds)

    while dividers.count < frames.dividers.count {
      let divider = DividerView(frame: .zero)
      addSubview(divider)
      dividers.append(divider)
    }
    while dividers.count > frames.dividers.count {
      dividers.removeLast().removeFromSuperview()
    }

    // Placed in the tree's reading order, which is the order of its leaves.
    for (placement, key) in zip(frames.panes, tree.leaves.map(\.key)) {
      guard let chrome = chrome(for: key) else { continue }
      chrome.frame = placement.frame
      chrome.needsLayout = true
    }
    for (divider, placement) in zip(dividers, frames.dividers) {
      // Rebound every pass rather than once at pooling, because what a divider is named by is a
      // property of where it landed this time - the pool holds views and nothing else.
      let path = placement.path
      divider.onDrag = { [weak self] ratio in
        self?.dividerPositions.send(tab: tab, path: path, ratio: ratio)
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
