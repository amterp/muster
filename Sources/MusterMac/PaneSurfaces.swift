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
/// **Two regions showing one tab used to build two surfaces**, and only one of them can have
/// the terminal - the other prints herdr's refusal and cannot be closed. The core no longer
/// opens two such regions, and a store keyed by pane cannot produce two surfaces even if it
/// ever does again (kan a_2Ht74jTXV).
///
/// **A pane's border and badge used to be found by walking the regions**, which reached only
/// what was on screen. Keyed by pane, a parked pane keeps its state painted, so a tab switched
/// back to is right on the first frame rather than on the next agent transition.
///
/// What it costs is what the card priced: one `muster-bridge`, one ssh channel and one herdr
/// client per pane, held for as long as the pane exists, rather than per pane on screen. A
/// window of fifteen agents holds fifteen of each. They are released when the daemon stops
/// holding the pane, so the cost scales with panes rather than with switches.
@MainActor
public final class PaneSurfaces {
  /// Gives a pane's chrome a surface, and the bridge that feeds it.
  ///
  /// Injected because the real one needs a GPU, a libghostty runtime and a subprocess, and
  /// what this class decides - which panes have surfaces, which region is showing each, when
  /// one is let go - is worth testing without any of the three.
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

  /// Where a chrome waits while no region is showing its pane.
  ///
  /// Hidden, and inside the window rather than held with no superview. A surface is handed to
  /// libghostty as a view, and keeping that view in a window for its whole life is the state
  /// this code has always run in - a parked chrome that had left the hierarchy would be a new
  /// one, and the difference is only visible on a GPU.
  private let parking = NSView(frame: .zero)

  private var held: [PaneKey: Held] = [:]

  public init(parkedIn container: NSView, startPane: @escaping StartPane) {
    self.startPane = startPane
    parking.isHidden = true
    container.addSubview(parking)
  }

  public func chrome(for key: PaneKey) -> PaneChrome? { held[key]?.chrome }

  /// The chrome for a pane a region is about to show, and whether it needs a surface.
  ///
  /// The caller adds it to the region, lays the region out, and only then starts what this
  /// said was new - because libghostty is handed a view and sizes its surface from it, so a
  /// surface created against a zero-sized view is a PTY told it has no columns.
  ///
  /// A pane whose control socket moved is torn down and built again rather than reused. Left
  /// alone its bridge would keep painting into a socket nothing is listening on and swallow
  /// every keystroke, which is the symptom that has cost this project the most time.
  public func borrow(
    daemonID: String, leaf: PaneTree.Leaf, focus: @escaping (String) -> Void,
    scroll: @escaping (String, String, Double) -> Void
  ) -> (chrome: PaneChrome, isNew: Bool) {
    let key = PaneKey(daemon: daemonID, pane: leaf.paneID)
    if let existing = held[key] {
      if existing.controlSocketPath == leaf.controlSocketPath {
        return (existing.chrome, false)
      }
      Core.info(
        "pane.surface.rebuilt",
        [
          "pane": leaf.paneID,
          "reason": "its control socket changed, so its bridge was dialing a closed listener",
        ])
      release(key)
    }

    let chrome = PaneChrome(frame: .zero, surface: SurfaceView(frame: .zero))
    chrome.attach(paneID: leaf.paneID)
    chrome.onFocusRequested = focus
    chrome.onScrollRequested = scroll
    held[key] = Held(chrome: chrome, controlSocketPath: leaf.controlSocketPath)
    return (chrome, true)
  }

  /// Starts the bridge for a pane that has just been given a chrome and laid out.
  public func start(
    daemonID: String, transport: WindowContents.Region.Transport?, backendSocket: String?,
    chrome: PaneChrome, leaf: PaneTree.Leaf
  ) {
    startPane(daemonID, transport, backendSocket, chrome, leaf)
  }

  /// Takes back every chrome no region is showing, and keeps it alive off screen.
  ///
  /// In one pass over the whole window rather than per region, and it has to be: a pane that
  /// moved from one region to another is claimed by the second and given up by the first, and
  /// a region that parked its own departures as it applied would take back a chrome the region
  /// beside it had already adopted - whenever the two applied in that order.
  public func park(everythingBut onScreen: Set<PaneKey>) {
    for (key, entry) in held where !onScreen.contains(key) {
      guard entry.chrome.superview !== parking else { continue }
      parking.addSubview(entry.chrome)
    }
  }

  /// Lets go of every parked pane the daemons no longer hold.
  ///
  /// Driven by the roster, which is the one message naming every pane on every attached
  /// daemon whether or not a region is showing it. A pane that closed while its tab was off
  /// screen leaves this way; without it, a window that visits fifteen tabs holds fifteen
  /// tabs' worth of bridges until it quits.
  ///
  /// Only parked panes, never one a region is showing. A roster that arrives empty - a daemon
  /// mid-reconnect, a publish between arrangements - would otherwise tear down the window.
  public func release(everythingBut alive: Set<PaneKey>) {
    for (key, entry) in held
    where !alive.contains(key) && entry.chrome.superview === parking {
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
