import AppKit

/// Which side of a pane a dragged pane is dropped on, as the core spells a side.
public enum DropSide: String, Sendable, CaseIterable {
  case left, right, up, down
}

/// The rules for dragging a pane onto another. What starts one is the pane's header
/// (`PaneHeaderView`).
///
/// Ghostty's, measured off its macOS app (`SurfaceGrabHandle.swift`, `TerminalSplitTreeView.swift`)
/// so that somebody arriving from the terminal Muster embeds grabs a pane where they already
/// would. Pure, for the reason `SidebarModel` is: a decision made inside `draggingUpdated` is a
/// decision no test can reach.
///
/// Every rect here is in a pane's own AppKit coordinates, origin at the bottom left, because that
/// is what `PaneChrome` is drawn in.
public enum PaneDrop {
  /// What a dragged pane is carried as. The agent list's own type and payload, so a pane dragged
  /// by its handle can be dropped on a row or a tab caption there, and a row dragged out of the
  /// list can be dropped on a pane.
  public static let type = NSPasteboard.PasteboardType("dev.muster.pane")

  /// A key rather than a place in a list, so it still names the pane after the roster that drew
  /// the drag has been rebuilt under it.
  public static func payload(_ pane: PaneKey) -> String { "\(pane.daemon)\t\(pane.pane)" }

  public static func pane(fromPayload carried: String) -> PaneKey? {
    let parts = carried.split(separator: "\t", maxSplits: 1, omittingEmptySubsequences: false)
    guard parts.count == 2, !parts[0].isEmpty, !parts[1].isEmpty else { return nil }
    return PaneKey(daemon: String(parts[0]), pane: String(parts[1]))
  }

  /// The side of the pane nearest the pointer: its diagonals cut it into four triangles.
  ///
  /// Measured as a share of the width and the height rather than in points, so the diagonals
  /// run corner to corner in a pane of any shape, and a tie goes left, right, up, down. No middle
  /// zone, as in Ghostty: a drop always names a side.
  public static func zone(at point: CGPoint, in size: CGSize) -> DropSide {
    guard size.width > 0, size.height > 0 else { return .right }
    let across = point.x / size.width
    let up = point.y / size.height
    let nearest = min(across, 1 - across, up, 1 - up)
    if nearest == across { return .left }
    if nearest == 1 - across { return .right }
    if nearest == 1 - up { return .up }
    return .down
  }

  /// The half of the pane a drop on this side would hand to the dragged pane, which is what the
  /// overlay fills while the pointer is there.
  public static func overlay(for side: DropSide, in bounds: CGRect) -> CGRect {
    switch side {
    case .left: bounds.divided(atDistance: bounds.width / 2, from: .minXEdge).slice
    case .right: bounds.divided(atDistance: bounds.width / 2, from: .maxXEdge).slice
    case .up: bounds.divided(atDistance: bounds.height / 2, from: .maxYEdge).slice
    case .down: bounds.divided(atDistance: bounds.height / 2, from: .minYEdge).slice
    }
  }

  /// Whether a pane may be dropped on this one.
  ///
  /// Not on itself, which would be a move that goes nowhere, and not from another window, which
  /// is another process holding its own panes. From another machine, yes: the pane stays a
  /// process on its own machine, and what the drop changes is where it sits in the tab's tree,
  /// which spans machines (MIP-8, section 6).
  public static func accepts(_ dragged: PaneKey, onto target: PaneKey, fromThisWindow: Bool)
    -> Bool
  {
    fromThisWindow && dragged != target
  }
}

/// The half of a pane a drop would hand over, filled while a dragged pane is over that side.
///
/// Transparent to the mouse for the reason the badge is: it is drawn over a terminal, and a
/// view that took the click under it would make the pane unclickable for as long as it lingered.
@MainActor
final class PaneDropOverlay: NSView {
  override init(frame: NSRect) {
    super.init(frame: frame)
    wantsLayer = true
    isHidden = true
    // Not animated, as in Ghostty: the half that lights is the side the pointer is nearest now.
    layer?.actions = ["position": NSNull(), "bounds": NSNull(), "hidden": NSNull()]
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  func show(_ rect: CGRect) {
    // The focus ring's color, which is the accent unless the config names another: Ghostty
    // fills the same half in the accent at this strength.
    layer?.backgroundColor = PaneAppearance.focusColor.withAlphaComponent(0.3).cgColor
    frame = rect
    isHidden = false
  }

  func hide() {
    isHidden = true
  }
}

extension NSView {
  /// Whether a drag started in this view's window.
  ///
  /// Its source's window, not merely whether it has a source: a drag from another process has
  /// none this one can see, and a drag from another window of this process has one - so a source
  /// alone stopped telling the two apart once every window shared a process.
  func cameFromThisWindow(_ info: NSDraggingInfo) -> Bool {
    guard let source = info.draggingSource as? NSView else { return false }
    return source.window === window
  }
}
