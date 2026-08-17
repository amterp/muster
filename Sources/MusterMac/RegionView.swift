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
  /// Gives a pane's chrome a surface, and the bridge that feeds it.
  ///
  /// Injected because the real one needs a GPU, a libghostty runtime and a subprocess, and
  /// what this class decides - which panes exist, where they go, which one has the keyboard -
  /// is worth testing without any of the three.
  ///
  /// Called once the chrome is in the window and has been laid out, because libghostty is
  /// handed a view and sizes its surface from it: a surface created against a zero-sized view
  /// is a PTY told it has no columns.
  public typealias StartPane =
    @MainActor (
      _ daemonID: String, _ transport: WindowContents.Region.Transport?,
      _ backendSocket: String?, _ chrome: PaneChrome, _ pane: PaneTree.Leaf
    ) -> Void

  private struct Held {
    let chrome: PaneChrome

    /// What its bridge was pointed at when it was built. A pane whose socket changed needs a
    /// new bridge, and a bridge is spawned by its surface's command - so it needs a new
    /// surface too.
    let controlSocketPath: String?
  }

  private let startPane: StartPane
  private var held: [String: Held] = [:]
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

  public init(frame: NSRect, startPane: @escaping StartPane) {
    self.startPane = startPane
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

  public func chrome(for paneID: String) -> PaneChrome? { held[paneID]?.chrome }

  /// Renders a region, keeping every surface the new tree still names.
  ///
  /// A nil tree leaves what is on screen alone. The daemon has not said how this tab is
  /// arranged - it publishes the tree on its own event, which may follow the panes it names -
  /// and tearing surfaces down for a moment that resolves in milliseconds is a flicker on
  /// every split.
  public func apply(_ region: WindowContents.Region, focused: Bool) {
    regionID = region.id
    daemonID = region.daemon
    transport = region.transport
    backendSocket = region.backendSocket
    tab = region.tab
    guard let tree = region.tree else { return }
    self.tree = tree
    let fresh = reconcilePanes(tree.leaves)
    // Laid out before the surfaces are made, so each one is handed a view that already has
    // the size it will keep.
    needsLayout = true
    layoutSubtreeIfNeeded()
    for leaf in fresh {
      guard let chrome = held[leaf.paneID]?.chrome else { continue }
      startPane(daemonID, transport, backendSocket, chrome, leaf)
    }
    apply(keyboardPane: focused ? region.keyboardPane : nil)
  }

  /// Builds what the tree names and lets go of what it does not.
  ///
  /// Returns the panes that need a surface: the ones that were not here, and the ones whose
  /// old surface was just thrown away.
  private func reconcilePanes(_ leaves: [PaneTree.Leaf]) -> [PaneTree.Leaf] {
    var fresh: [PaneTree.Leaf] = []
    for leaf in leaves {
      if let existing = held[leaf.paneID] {
        guard existing.controlSocketPath != leaf.controlSocketPath else { continue }
        // The socket moved, so this pane's bridge is dialing somewhere nothing is listening.
        // Left alone the pane would keep painting and swallow every keystroke, which is the
        // symptom that has cost this project the most time.
        Core.info(
          "pane.surface.rebuilt",
          [
            "pane": leaf.paneID,
            "reason": "its control socket changed, so its bridge was dialing a closed listener",
          ])
        existing.chrome.removeFromSuperview()
      }
      let chrome = PaneChrome(frame: bounds, surface: SurfaceView(frame: bounds))
      chrome.attach(paneID: leaf.paneID)
      chrome.onFocusRequested = { [weak self] paneID in
        Core.focus(daemonID: self?.daemonID ?? "", paneID: paneID)
      }
      chrome.onScrollRequested = { [weak self] paneID, direction, delta in
        Core.scroll(
          daemonID: self?.daemonID ?? "", paneID: paneID, direction: direction, delta: delta)
      }
      addSubview(chrome)
      held[leaf.paneID] = Held(chrome: chrome, controlSocketPath: leaf.controlSocketPath)
      fresh.append(leaf)
    }

    let named = Set(leaves.map(\.paneID))
    for (paneID, gone) in held where !named.contains(paneID) {
      // Dropping the chrome drops its surface, which ends the command that surface spawned -
      // so the pane's bridge exits here rather than being left dialing a window that has
      // forgotten it.
      gone.chrome.removeFromSuperview()
      held.removeValue(forKey: paneID)
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
    for (paneID, pane) in held {
      pane.chrome.apply(focused: paneID == keyboardPane)
    }
    guard let keyboardPane, let chrome = held[keyboardPane]?.chrome, let window else { return }
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
      guard let chrome = held[placement.paneID]?.chrome else { continue }
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
