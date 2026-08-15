import AppKit
import MusterRenderer

/// What a pane's renderer has to answer for a view to drive it.
///
/// Declared here rather than in `MusterRenderer` because it is the shell's list of demands,
/// not the adapter's offer: a second renderer earns its place by satisfying this, in an
/// extension as short as the one below.
///
/// It also puts a seam where the tests need one. A real `Surface` wants a GPU, a window and a
/// libghostty runtime, so a view that talks to one directly can only be exercised by launching
/// the app - which is how copy shipped with nothing asserting that a drag reaches the grid or
/// that ⌘C reaches the clipboard.
@MainActor
public protocol PaneSurface {
  func setSize(width: UInt32, height: UInt32)
  func setFocus(_ focused: Bool)

  /// Where the pointer is, in the surface's own coordinates - measured from its top left,
  /// which is not where AppKit measures from. The caller converts.
  func mouseMoved(to point: NSPoint, modifiers: NSEvent.ModifierFlags)

  func leftMouse(pressed: Bool, modifiers: NSEvent.ModifierFlags)

  /// What is selected in this pane, or nil when nothing is.
  var selectedText: String? { get }
}

extension Surface: PaneSurface {}
