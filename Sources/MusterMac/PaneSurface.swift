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
public protocol PaneSurface: AnyObject {
  func setSize(width: UInt32, height: UInt32)
  func setFocus(_ focused: Bool)

  /// Sizes this pane's text, in points away from what the configuration asked for. Zero puts it
  /// back. An offset rather than a size because the size it offsets from may be the renderer's
  /// own, and this side of the seam never learns what that is.
  ///
  /// Answers with whatever the renderer would not do, which is empty in every ordinary case.
  /// A renderer that cannot size text is a real answer rather than an error - what it costs is
  /// one chord, and the window is otherwise fine - so this is a line for the log rather than a
  /// throw.
  @discardableResult
  func setFontSizeOffset(_ points: Int32) -> [String]

  /// Marks every occurrence of some text on this pane's screen, and `nil` clears the marks.
  ///
  /// The renderer's whole part in find, and it is drawing rather than searching. The core
  /// holds the answer to "how many matches are there" - it read the pane's history from the
  /// daemon to get it - and this asks for the ones now in view to be shown. A renderer whose
  /// own search covered more than the screen would still be answering a narrower question
  /// than the core's, because a surface here is repainted from a frame stream and holds no
  /// history at all (`architecture.md`, control plane and data plane).
  ///
  /// Answers with whatever it would not do, like `setFontSizeOffset` and for the same reason:
  /// a renderer that cannot mark text is a real answer rather than an error. What it costs is
  /// the highlight, and the counter and the scrolling are unaffected - so this is a line for
  /// the log rather than a throw.
  @discardableResult
  func highlight(_ text: String?) -> [String]

  /// Called when the command this surface is running exits, which for a pane means its
  /// bridge is gone. Settable rather than reported once, because whoever owns the surface is
  /// not who needs to know.
  var onProcessExited: (@MainActor (Bool) -> Void)? { get set }

  /// Where the pointer is, in the surface's own coordinates - measured from its top left,
  /// which is not where AppKit measures from. The caller converts.
  func mouseMoved(to point: NSPoint, modifiers: NSEvent.ModifierFlags)

  func leftMouse(pressed: Bool, modifiers: NSEvent.ModifierFlags)

  /// What is selected in this pane, or nil when nothing is.
  var selectedText: String? { get }

  /// How big one cell is, in backing pixels, or nil before the surface has been sized.
  ///
  /// Only a live surface knows: it is the font's own measurement and it moves with the text
  /// size. The core needs it to read a `resize_step` written in points, since the daemon
  /// resizes a grid and something has to divide.
  var cellPixelSize: (width: UInt32, height: UInt32)? { get }
}

extension Surface: PaneSurface {}
