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

  /// Whether the window holding this surface can be seen. A surface that cannot stops drawing,
  /// which is the renderer's work for a window that is hidden, covered or on a locked screen.
  func setOcclusion(visible: Bool)

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

  /// Searches this pane for a needle, replacing any search already running, and `nil` ends it.
  ///
  /// The renderer's whole part in find, and all of it: the surface holds the pane's history, so
  /// it counts, marks and scrolls by itself, and reports what it found through `onSearch`.
  ///
  /// Answers with whatever it would not do, like `setFontSizeOffset` and for the same reason: a
  /// renderer that cannot search is a real answer rather than an error, so this is a line for the
  /// log rather than a throw.
  @discardableResult
  func search(_ needle: String?) -> [String]

  /// Moves to the next match of the running search, or the previous one.
  func navigateSearch(next: Bool)

  /// Called when the running search has counted its matches or moved to one.
  var onSearch: (@MainActor (SearchReport) -> Void)? { get set }

  /// Called when somebody cmd-clicks a link in the pane, asking for it to be opened.
  var onOpenLink: (@MainActor (OpenedLink) -> Void)? { get set }

  /// Called when the pointer over the pane should change shape.
  var onPointerShape: (@MainActor (PointerShape) -> Void)? { get set }

  /// Called with the link under the pointer as it arrives over one, and nil as it leaves.
  var onHoverLink: (@MainActor (String?) -> Void)? { get set }

  /// Called when the command this surface is running exits, which for a pane means its
  /// bridge is gone. Settable rather than reported once, because whoever owns the surface is
  /// not who needs to know.
  var onProcessExited: (@MainActor (Bool) -> Void)? { get set }

  /// A key the pane's program was given, handed to the surface for what it does on a keystroke
  /// by itself. `committed` is an input method's text for it, and `composing` whether one was
  /// or still is composing.
  func pressKey(_ event: NSEvent, committed: String?, composing: Bool)
  func releaseKey(_ event: NSEvent)

  /// A modifier pressed or released on its own.
  func changeModifiers(_ event: NSEvent)

  /// Where the pointer is, in the surface's own coordinates - measured from its top left,
  /// which is not where AppKit measures from. The caller converts.
  func mouseMoved(to point: NSPoint, modifiers: NSEvent.ModifierFlags)

  /// Presses or releases a button, by AppKit's `buttonNumber` for it, and says whether the
  /// renderer consumed it.
  @discardableResult
  func mouseButton(_ number: Int, pressed: Bool, modifiers: NSEvent.ModifierFlags) -> Bool

  /// Whether the program in this pane has asked for the mouse.
  var mouseCaptured: Bool { get }

  /// Scrolls the surface's own history, or lets it answer as the pane's modes say. Deltas are
  /// positive right and up, and `momentum` is in libghostty's numbering.
  func scroll(dx: Double, dy: Double, precise: Bool, momentum: UInt32)

  /// Carries out one of Ghostty's surface-local binding actions - scrolling, jumping between
  /// prompts, selecting everything - and answers with it if the renderer did not.
  @discardableResult
  func perform(_ action: SurfaceAction) -> [String]

  /// Draws what an input method is composing at the pane's cursor, or clears it with nil. Never
  /// sent to the pane: only what the method commits is.
  func setPreedit(_ text: String?)

  /// The cell under the pane's cursor, in points from the surface's top left, with `y` at the
  /// cell's bottom edge. Where an input method's candidate window goes.
  var cursorCell: NSRect { get }

  /// What is selected in this pane, or nil when nothing is.
  var selectedText: String? { get }

  /// How big one cell is, in backing pixels, or nil before the surface has been sized.
  ///
  /// Only a live surface knows: it is the font's own measurement and it moves with the text
  /// size. The core needs it to read a `resize_step` written in points, since the daemon
  /// resizes a grid and something has to divide.
  var cellPixelSize: (width: UInt32, height: UInt32)? { get }

  /// How much of the surface its cells cover, in backing pixels, or nil before it is sized.
  var textPixelSize: (width: UInt32, height: UInt32)? { get }

  /// The space between the pane's text and the surface's top and left edges, in points, as it
  /// was when the surface was made: a reloaded `pane_padding` reaches new surfaces only.
  var padding: Double { get }
}

extension Surface: PaneSurface {}
