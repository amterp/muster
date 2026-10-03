import AppKit

/// Which side of a pane a dragged pane is dropped on, as the core spells a side.
public enum DropSide: String, Sendable, CaseIterable {
  case left, right, up, down
}

/// The rules for dragging a pane onto another, and where the handle that starts one sits.
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
  /// Not on itself, which would be a move that goes nowhere. Not from another machine: a pane
  /// is a process its daemon owns, so it can sit beside another machine's panes only as that
  /// machine's part of a tab, which is what dropping it on the tab's caption in the agent list
  /// does. And not from another window, which is another process holding its own panes.
  public static func accepts(_ dragged: PaneKey, onto target: PaneKey, fromThisWindow: Bool)
    -> Bool
  {
    fromThisWindow && dragged != target && dragged.daemon == target.daemon
  }

  /// The grab area, as Ghostty sizes it.
  public static let handleSize = CGSize(width: 80, height: 12)

  /// Where the handle sits: centered at the top of the pane, over the top edge of its terminal.
  public static func handleFrame(in bounds: CGRect) -> CGRect {
    CGRect(
      x: bounds.midX - handleSize.width / 2, y: bounds.maxY - handleSize.height,
      width: handleSize.width, height: handleSize.height)
  }

  /// The band across the top of a pane where the handle draws itself: the top fifth, and never
  /// less than the handle. The handle is there to grab everywhere else too, only quietly, so a
  /// pane nobody is reaching for carries nothing over its text.
  public static func handleBand(in bounds: CGRect) -> CGRect {
    let height = min(bounds.height, max(handleSize.height, bounds.height * 0.2))
    return CGRect(x: bounds.minX, y: bounds.maxY - height, width: bounds.width, height: height)
  }
}

/// What a pane is dragged by.
///
/// The only way to start dragging a pane, and that is deliberate. Everywhere else in a pane a
/// drag is the program's - a selection, or a report to a program that asked for the mouse - and
/// a modifier that turned it into a move would be one more key a program in the pane can no
/// longer have. A press here never reaches the terminal underneath.
@MainActor
final class PaneGrabHandle: NSView, NSDraggingSource {
  /// The pane this drags. Nil hides the handle, for a chrome with no pane behind it.
  var pane: PaneKey? {
    didSet { isHidden = pane == nil }
  }

  /// Called on a press, which asks for the keyboard here as a click on the pane does - so the
  /// pane that was moved is the one being typed into, as Ghostty leaves it.
  var onPressed: (() -> Void)?

  /// Whether the pointer is in the band across the top of the pane, which the pane tracks.
  var pointerInBand = false {
    didSet { if pointerInBand != oldValue { needsDisplay = true } }
  }

  private var hovering = false
  private(set) var dragging = false

  /// Whether the symbol is drawn: near the top of the pane, over the handle, or mid-drag.
  var iconShown: Bool { pointerInBand || hovering || dragging }

  override init(frame: NSRect) {
    super.init(frame: frame)
    isHidden = true
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  /// A press into a window that is not key still grabs, as a click into one still picks a pane.
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  override func mouseDown(with event: NSEvent) {
    onPressed?()
  }

  override func mouseDragged(with event: NSEvent) {
    guard !dragging, let pane, let chrome = superview else { return }
    let item = NSPasteboardItem()
    item.setString(PaneDrop.payload(pane), forType: PaneDrop.type)
    let dragged = NSDraggingItem(pasteboardWriter: item)
    let preview = Self.preview(for: chrome.bounds.size)
    let at = convert(event.locationInWindow, from: nil)
    dragged.setDraggingFrame(
      CGRect(
        x: at.x - preview.size.width / 2, y: at.y - preview.size.height / 2,
        width: preview.size.width, height: preview.size.height),
      contents: preview)
    dragging = true
    needsDisplay = true
    let session = beginDraggingSession(with: [dragged], event: event, source: self)
    // A drop nowhere ends there, as Ghostty's does, rather than sliding back to remind
    // somebody of a drag they already know they abandoned.
    session.animatesToStartingPositionsOnCancelOrFail = false
  }

  func draggingSession(
    _ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext
  ) -> NSDragOperation {
    context == .withinApplication ? .move : []
  }

  func draggingSession(_ session: NSDraggingSession, movedTo screenPoint: NSPoint) {
    NSCursor.closedHand.set()
  }

  func draggingSession(
    _ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation
  ) {
    dragging = false
    needsDisplay = true
  }

  override func resetCursorRects() {
    addCursorRect(bounds, cursor: .openHand)
  }

  override func updateTrackingAreas() {
    trackingAreas.forEach(removeTrackingArea)
    addTrackingArea(
      NSTrackingArea(
        rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
        owner: self))
    super.updateTrackingAreas()
  }

  override func mouseEntered(with event: NSEvent) {
    hovering = true
    needsDisplay = true
  }

  override func mouseExited(with event: NSEvent) {
    hovering = false
    needsDisplay = true
  }

  override func draw(_ dirty: NSRect) {
    guard iconShown,
      let symbol = NSImage(systemSymbolName: "ellipsis", accessibilityDescription: "Move pane")?
        .withSymbolConfiguration(.init(pointSize: 10, weight: .semibold))
    else { return }
    let tint = NSColor.labelColor.withAlphaComponent(hovering || dragging ? 0.8 : 0.3)
    let tinted = NSImage(size: symbol.size, flipped: false) { rect in
      symbol.draw(in: rect)
      tint.set()
      rect.fill(using: .sourceAtop)
      return true
    }
    // A little above the middle, as Ghostty's sits, so it reads as belonging to the edge.
    let origin = NSPoint(
      x: bounds.midX - symbol.size.width / 2, y: bounds.midY - symbol.size.height / 2 + 2)
    tinted.draw(at: origin, from: .zero, operation: .sourceOver, fraction: 1)
  }

  /// What follows the pointer: the pane's outline at a fifth of its size.
  ///
  /// An outline rather than Ghostty's snapshot of the pane, because a terminal here draws into a
  /// Metal layer that `cacheDisplay` cannot read, and a snapshot would come out blank.
  static func preview(for size: CGSize) -> NSImage {
    let scaled = CGSize(width: max(size.width * 0.2, 24), height: max(size.height * 0.2, 16))
    return NSImage(size: scaled, flipped: false) { rect in
      let outline = NSBezierPath(roundedRect: rect.insetBy(dx: 1, dy: 1), xRadius: 4, yRadius: 4)
      PaneAppearance.focusColor.withAlphaComponent(0.25).setFill()
      outline.fill()
      PaneAppearance.focusColor.setStroke()
      outline.lineWidth = 2
      outline.stroke()
      return true
    }
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
