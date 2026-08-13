import AppKit

/// The line between two panes, and the handle that moves it.
///
/// Deliberately not `NSSplitView`. That class owns its divider positions, so every event the
/// daemon sends would be a fight over who decides where the line is - and the daemon wins by
/// construction here (`architecture.md`, view = f(daemon state)). What is left once that
/// argument is settled is this: a strip that turns a drag into an intent and then waits, like
/// every other thing the user can do.
///
/// So a drag does not move anything locally. It asks, the daemon answers on its own event,
/// and the divider lands where the daemon put it. The round trip is well under a frame, and
/// the alternative is a window that shows an arrangement no daemon agreed to.
@MainActor
final class DividerView: NSView {
  /// The turns from the region's root down to this split, which is how a divider is named to
  /// a daemon - it has no id of its own.
  var path: [Bool] = []
  var axis: SplitAxis = .columns

  /// The rectangle the two children share, in the region's coordinates.
  var area: CGRect = .zero

  /// Asks for a new share for the first child. Called while dragging, not at the end: a
  /// divider that only moved on mouse-up would look stuck for the length of the gesture.
  var onDrag: ((_ path: [Bool], _ ratio: CGFloat) -> Void)?

  /// The last ratio asked for, so that a pointer moving along the divider rather than across
  /// it does not send a request per frame for a position that has not changed.
  private var asked: CGFloat?

  /// How much a drag has to move before it is worth a round trip. Below this the daemon would
  /// answer with a layout indistinguishable from the one on screen.
  private static let sensitivity: CGFloat = 0.002

  override init(frame: NSRect) {
    super.init(frame: frame)
    wantsLayer = true
    layer?.backgroundColor = NSColor.separatorColor.cgColor
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  override func resetCursorRects() {
    addCursorRect(bounds, cursor: axis == .columns ? .resizeLeftRight : .resizeUpDown)
  }

  override func mouseDown(with event: NSEvent) {
    asked = nil
  }

  override func mouseDragged(with event: NSEvent) {
    guard let superview else { return }
    let point = superview.convert(event.locationInWindow, from: nil)
    let ratio = PaneTree.ratio(at: point, in: area, axis: axis)
    guard asked.map({ abs(ratio - $0) >= DividerView.sensitivity }) ?? true else { return }
    asked = ratio
    onDrag?(path, ratio)
  }
}
