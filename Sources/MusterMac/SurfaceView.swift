import AppKit
import MusterRenderer

/// Hosts one libghostty surface, and feeds the pane it mirrors.
///
/// libghostty attaches its own Metal layer to whatever NSView it is handed, so this view
/// draws nothing itself. What it does own is input: the surface is a renderer, and nothing
/// typed here goes through it (architecture.md, the renderer seam).
///
/// A library rather than part of the executable, because a view is reachable by a test and
/// an executable's top-level code is not. `NSEvent.keyEvent(with:...)` builds a keystroke
/// with no app, window or run loop behind it, so what happens when you type is assertable
/// here - which is how the bug that sent every key twice would have been caught.
@MainActor
public final class SurfaceView: NSView, NSMenuItemValidation {
  private var surface: (any PaneSurface)?

  /// Where copy and paste meet the rest of the machine. Settable so a test can hand the view
  /// a pasteboard of its own rather than reaching into whatever the developer last copied.
  public var pasteboard: NSPasteboard = .general

  /// Whether this view has a pane to type into. A bare `muster` does not - it is the
  /// renderer check - and a view that sent keystrokes anyway would fill the log with
  /// refusals for a state that is expected.
  private var isTypeable = false

  /// The composition in progress, if any. Its length is the whole of "is the input method
  /// still working", which is what `NSTextInputClient` asks about constantly.
  private var markedText = NSMutableAttributedString()

  /// What the input method committed while the current `keyDown` was being interpreted.
  ///
  /// Held rather than sent, because at the moment `insertText` runs it is not yet known
  /// whether this is a composition finishing or AppKit simply handing back the character
  /// that was typed. `CompositionArbiter` decides once both facts are in.
  private var committedText: String?

  /// Whether `insertText` is being called from inside `keyDown`.
  private var interpretingKeyEvent = false

  /// Called when this view is clicked, meaning the user wants the keyboard here.
  ///
  /// A click is the primitive for picking a pane out of fifteen, and it is not first
  /// responder handling: which pane the keyboard feeds is the core's answer, so a click asks
  /// rather than takes. The responder move follows from the view the core publishes back.
  ///
  /// Mouse events do not otherwise reach the pane yet - a pane's mouse mode is not readable,
  /// so an encoded click would be a guess (kan a_27CTgqqdv) - which leaves the gesture free
  /// to mean this and nothing else.
  public var onClick: (@MainActor () -> Void)?

  /// Called when the wheel moves over this view, meaning the user wants *this* pane scrolled.
  ///
  /// Reported rather than sent, for the same reason a click is: the view under the pointer
  /// knows the gesture happened and nothing else, and which pane that is belongs to the chrome
  /// around it. AppKit hit-tests `scrollWheel` to the view the pointer is over, so this fires
  /// on the right surface whether or not it is the one with the keyboard - which is the whole
  /// of the feature.
  public var onScroll: (@MainActor (_ direction: String, _ delta: Double) -> Void)?

  public override init(frame: NSRect) {
    super.init(frame: frame)
    // Layer-backed before the surface is created, and on the main thread. libghostty's
    // renderer runs on its own thread and wants a layer waiting for it; making it ask
    // AppKit for one from there trips a dispatch-queue assertion. ghostty's own app never
    // hits this because SwiftUI has already made its view hierarchy layer-backed.
    wantsLayer = true
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  public override var acceptsFirstResponder: Bool { true }

  /// Called when this pane's bridge exits, so the window can report which pane it was.
  ///
  /// Separate from stopping the keystrokes, which happens here regardless: a view that kept
  /// sending them would fill the log with one refusal per key for a pane nobody can reach.
  /// Which pane and which daemon this was is the window's to know, not a surface's.
  public var onProcessExited: (@MainActor (Bool) -> Void)?

  public func attach(_ surface: any PaneSurface, typeable: Bool) {
    self.surface = surface
    surface.onProcessExited = { [weak self] processAlive in
      self?.paneEnded(processAlive: processAlive)
    }
    attach(typeable: typeable)
    surface.setSize(
      width: UInt32(bounds.width * (window?.backingScaleFactor ?? 2)),
      height: UInt32(bounds.height * (window?.backingScaleFactor ?? 2)))
  }

  /// How big one cell is in points, which is what the core divides a `resize_step` by.
  ///
  /// Points rather than the backing pixels the renderer answers in, because every dimension a
  /// config file names - `pane_padding`, `[font] size` - is points, and two length keys in one
  /// file that mean different things is a trap. The scale factor is read here for the same
  /// reason `setSize` writes it here: AppKit keeps it on the window, which the renderer has no
  /// business knowing about.
  public var cellPointSize: (width: Float, height: Float)? {
    guard let pixels = surface?.cellPixelSize else { return nil }
    let scale = Float(window?.backingScaleFactor ?? 2)
    guard scale > 0 else { return nil }
    return (Float(pixels.width) / scale, Float(pixels.height) / scale)
  }

  /// Sizes this pane's text, once there is something rendering it.
  ///
  /// Silently nothing before a surface is attached, which is the ordinary case at launch: the
  /// window applies the offset to every pane it has, and a pane whose bridge has not started
  /// yet gets it when `attach` runs.
  @discardableResult
  public func setFontSizeOffset(_ points: Int32) -> [String] {
    surface?.setFontSizeOffset(points) ?? []
  }

  /// Marks what a find turned up, once there is something rendering this pane.
  ///
  /// Silently nothing before a surface is attached, on the same terms as sizing the text: a
  /// pane whose bridge has not started has nothing on screen to mark, and refusing would be
  /// reporting a renderer problem for a pane with no renderer yet.
  @discardableResult
  public func highlight(_ text: String?) -> [String] {
    surface?.highlight(text) ?? []
  }

  /// Points this view at a pane, independently of what renders it.
  ///
  /// Separate because the two are separate: a view will eventually be re-pointed at a
  /// different pane without its surface changing. It also lets a test drive the whole
  /// keystroke path with no GPU, no window and no daemon behind it.
  public func attach(typeable: Bool) {
    isTypeable = typeable
  }

  /// What this view does about its own bridge having exited.
  ///
  /// It stops typing into it. This surface is now a picture - libghostty paints its own
  /// "press any key to close the window" over it, and no key here will ever reach that -
  /// so every keystroke after this would reach a channel with nobody on the other end.
  ///
  /// Once, and only from typeable: a surface that was never a pane has nothing to stop.
  private func paneEnded(processAlive: Bool) {
    guard isTypeable else { return }
    isTypeable = false
    Core.warn(
      "pane.bridge.exited",
      [
        "process_alive": processAlive ? "true" : "false",
        "impact": "this pane renders whatever it last painted and takes no more keystrokes; "
          + "every other pane in the window is unaffected",
        "check": "a bridge.closed record from this pane's own bridge, which says why it "
          + "ended - most often the daemon no longer holds the pane",
      ])
    onProcessExited?(processAlive)
  }

  public override func setFrameSize(_ newSize: NSSize) {
    super.setFrameSize(newSize)
    let scale = window?.backingScaleFactor ?? 2
    surface?.setSize(width: UInt32(newSize.width * scale), height: UInt32(newSize.height * scale))
  }

  // No draw override. libghostty runs its own display link on its own renderer thread and
  // paints the layer it attached to this view, so a host that also calls
  // ghostty_surface_draw is a second painter racing the first.

  public override func keyDown(with event: NSEvent) {
    // The input method gets the event first: while it is composing, the keystrokes belong
    // to it - they select candidates and build characters - and the pane must see only
    // what it commits. interpretKeyEvents calls back into insertText or setMarkedText.
    let wasComposing = hasMarkedText()
    committedText = nil
    interpretingKeyEvent = true
    interpretKeyEvents([event])
    interpretingKeyEvent = false

    guard isTypeable else { return }
    // All three signals travel together and the core picks between them. Choosing here
    // would be the shell deciding what a keystroke means, and choosing *both* - the
    // committed text and the encoded key - is the bug that made `hello` arrive as
    // `hheelllloo`.
    Core.send(
      keyDown: event.musterKeyEvent(
        action: event.isARepeat ? "repeated" : "press", isComposing: hasMarkedText()),
      wasComposing: wasComposing,
      committed: committedText,
      stillComposing: hasMarkedText())
  }

  public override func keyUp(with event: NSEvent) {
    // Only reported when the pane asked for release events, which the encoder decides from
    // the mode profile. Sending it unconditionally is how a program that never asked ends
    // up with every keystroke twice.
    guard isTypeable else { return }
    Core.send(
      keyUp: event.musterKeyEvent(
        action: event.isARepeat ? "repeated" : "release", isComposing: hasMarkedText()))
  }

  /// A click into a window that is not key still picks the pane, rather than being spent
  /// activating the app. Fifteen panes make the alternative - click once to focus the window,
  /// again to pick the pane - a papercut on every switch back.
  public override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  // A drag makes a selection, and libghostty paints it from what is already on this screen.
  //
  // No mode has to be guessed for that, which is why copy works while reporting mouse buttons
  // to the program in the pane does not (kan a_27CTgqqdv): that needs the pane's mouse mode,
  // and frame diffs consume mode changes before this surface ever sees one
  // (`observations/herdr-0.8.0.md` section 2). The consequence worth knowing is that this
  // surface believes mouse reporting is always off, which is exactly what makes a drag mean
  // "select" here and never "click" over there.
  //
  // The one thing a drag does need from the daemon is where the pane is looking, so that the
  // selection can be counted from the bottom of the pane rather than from the top of this
  // screen - see the selection tracking below.

  public override func mouseDown(with event: NSEvent) {
    onClick?()
    // A new press starts a new selection, so whatever was pinned stops being what is on
    // screen. Dropped rather than replaced, because what replaces it is not known until the
    // button comes up - a click that never drags selects nothing.
    selection = .none
    pressedInGrid = cell(at: event)
    reportMouse(event, pressed: true)
  }

  public override func mouseDragged(with event: NSEvent) {
    reportMousePosition(event)
  }

  public override func mouseUp(with event: NSEvent) {
    reportMouse(event, pressed: false)
    guard let from = pressedInGrid, let to = cell(at: event), from != to else {
      // A click rather than a drag. libghostty may still have selected something - a double
      // click takes the word under it - and there is no way to read back what, so this is
      // left alone rather than pinned to cells it did not cover.
      pressedInGrid = nil
      return
    }
    pressedInGrid = nil
    selection = .madeOnScreen(GridSelection(from: from, to: to))
    onSelectionMade?()
  }

  // Keeping a selection on its own text.
  //
  // A pane is scrolled somewhere else. The daemon holds the history and answers a scroll by
  // repainting the screen in place, so this surface's own buffer never moves and a selection
  // - which libghostty pins to rows of that buffer - stays where it was drawn while the text
  // under it is rewritten (`observations/libghostty-9f9b8d1d.md` section 12).
  //
  // So the selection is remembered in the pane's own coordinates instead, and asked for again
  // wherever those have landed. Turning one into the other needs the pane's viewport, which
  // only the daemon knows and which is a round trip - hence the two steps: a drag reports
  // that it needs one, and whoever knows which pane this is fetches it and hands it back.

  /// The cell the current press started on, while a button is down.
  private var pressedInGrid: GridCell?

  private var selection: SelectionState = .none

  /// Called when a drag has ended and the cells it covered need pinning to the pane.
  ///
  /// A report rather than a request, for the reason a click and a wheel are reports: this view
  /// knows the gesture and the grid, and which pane it is drawing belongs to the chrome
  /// around it.
  public var onSelectionMade: (@MainActor () -> Void)?

  private enum SelectionState {
    case none
    /// A drag has ended and its cells are still screen cells, waiting for a viewport to be
    /// counted from the bottom of the pane instead.
    case madeOnScreen(GridSelection)
    /// Pinned to the pane, and placeable on whatever screen it is now showing.
    case pinnedToPane(GridSelection)
  }

  /// A cell of a grid, as a column and a row counted up from the bottom row.
  ///
  /// Up from the bottom because that is the direction everything else about a pane counts in -
  /// a match's row, a viewport's offset - and because it is the half of the conversion that
  /// does not need to know how tall anything is.
  struct GridCell: Equatable {
    var column: Int
    var rowsFromBottom: Int
  }

  struct GridSelection: Equatable {
    var from: GridCell
    var to: GridCell
  }

  /// Takes where the pane is looking, and does whatever the selection is waiting for.
  ///
  /// One entry point rather than two, because which of them applies is this view's own state
  /// and a caller choosing would have to know it. `movedSince` says whether the pane was asked
  /// to scroll between the drag ending and this answer arriving, which only matters while a
  /// selection is being pinned.
  ///
  /// A nil viewport is a core that would not say, which is the same news either way: the
  /// selection cannot be placed, so it comes off rather than staying over whatever is there.
  public func applyViewport(_ viewport: Core.Viewport?, movedSince: Bool = false) {
    switch selection {
    case .none: return
    case .madeOnScreen: anchorSelection(in: viewport, movedSince: movedSince)
    case .pinnedToPane: placeSelection(in: viewport)
    }
  }

  /// Pins a selection just made to the pane, so it can be found again after a scroll.
  ///
  /// The viewport has to be the one in force when the drag ended. A scroll in between makes
  /// this the wrong answer, and the honest response is to forget the selection and take it off
  /// the screen rather than pin it somewhere it never was.
  private func anchorSelection(in viewport: Core.Viewport?, movedSince: Bool) {
    guard case .madeOnScreen(let made) = selection else { return }
    guard let viewport, !movedSince else {
      selection = .none
      surface?.select(nil)
      return
    }
    let offset = Int(viewport.rowsFromBottom)
    selection = .pinnedToPane(
      GridSelection(
        from: GridCell(column: made.from.column, rowsFromBottom: made.from.rowsFromBottom + offset),
        to: GridCell(column: made.to.column, rowsFromBottom: made.to.rowsFromBottom + offset)))
  }

  /// Draws the pinned selection where the pane is now looking, or takes it off.
  ///
  /// An end that has scrolled past the screen is asked for beyond the edge it went past rather
  /// than clamped here, because libghostty clamps a position to its own grid - so a selection
  /// running up off the top starts at the first cell of the top row, which is what it means.
  private func placeSelection(in viewport: Core.Viewport?) {
    guard case .pinnedToPane(let pinned) = selection else { return }
    guard let viewport, let cellSize = cellPointSize else {
      surface?.select(nil)
      return
    }
    // The grid this view is drawing rather than the one the daemon reports, and the two can
    // differ for a frame while a resize settles. What is being worked out is a point on this
    // surface, so it has to be this surface's own height - the daemon's number is only good
    // for how far down the pane it is looking.
    let offset = Int(viewport.rowsFromBottom)
    guard let rows = gridRows else {
      surface?.select(nil)
      return
    }
    let lowest = pinned.from.rowsFromBottom < pinned.to.rowsFromBottom ? pinned.from : pinned.to
    let highest = lowest == pinned.from ? pinned.to : pinned.from
    // Wholly past one edge, so there is nothing of it to draw. The pin is kept: scrolling back
    // brings it into view again, which is what a selection anchored to text means.
    if highest.rowsFromBottom < offset || lowest.rowsFromBottom >= offset + rows {
      surface?.select(nil)
      return
    }
    surface?.select(
      SurfaceSelection(
        from: point(of: pinned.from, offset: offset, rows: rows, cellSize: cellSize),
        to: point(of: pinned.to, offset: offset, rows: rows, cellSize: cellSize)))
  }

  /// Whether anything is pinned, so a caller knows whether a scroll is worth a round trip.
  public var isTrackingSelection: Bool {
    if case .none = selection { return false }
    return true
  }

  /// Where a cell of the pane sits on this surface, in points from its top left.
  ///
  /// Deliberately off the surface when the cell is off the screen, and by a whole cell rather
  /// than a fraction of one: a renderer clamps it to the grid's edge, and half a cell over
  /// would round back onto the edge row instead of past it.
  private func point(
    of cell: GridCell, offset: Int, rows: Int, cellSize: (width: Float, height: Float)
  ) -> CGPoint {
    let row = rows - 1 - (cell.rowsFromBottom - offset)
    // The middle of the cell, so rounding cannot put it in its neighbour.
    return CGPoint(
      x: (CGFloat(cell.column) + 0.5) * CGFloat(cellSize.width),
      y: (CGFloat(row) + 0.5) * CGFloat(cellSize.height))
  }

  /// How many rows this view is drawing, or nil before anything has measured a cell.
  private var gridRows: Int? {
    guard let size = cellPointSize, size.height > 0 else { return nil }
    let rows = Int((frame.height / CGFloat(size.height)).rounded(.down))
    return rows > 0 ? rows : nil
  }

  /// Which cell of the grid an event landed on, counted up from the bottom row.
  private func cell(at event: NSEvent) -> GridCell? {
    guard let size = cellPointSize, size.width > 0, let rows = gridRows else { return nil }
    let point = convert(event.locationInWindow, from: nil)
    let row = Int(((frame.height - point.y) / CGFloat(size.height)).rounded(.down))
    let column = Int((point.x / CGFloat(size.width)).rounded(.down))
    return GridCell(
      column: max(0, column), rowsFromBottom: max(0, min(rows - 1, rows - 1 - row)))
  }

  private func reportMouse(_ event: NSEvent, pressed: Bool) {
    // The position first, because a button event is about wherever the pointer already is
    // and libghostty holds that separately - a press reported without one starts the
    // selection at the last place the pointer was seen.
    reportMousePosition(event)
    surface?.leftMouse(pressed: pressed, modifiers: event.modifierFlags)
  }

  private func reportMousePosition(_ event: NSEvent) {
    // AppKit measures this view from the bottom left and the surface measures itself from the
    // top left, so an unflipped position selects the mirror image of the drag.
    let point = convert(event.locationInWindow, from: nil)
    surface?.mouseMoved(
      to: NSPoint(x: point.x, y: frame.height - point.y), modifiers: event.modifierFlags)
  }

  public override func scrollWheel(with event: NSEvent) {
    // Scroll never becomes bytes here. It goes out as an intent, and the daemon answers it
    // against the pane's real modes - the one input-shaped thing Muster does not have to
    // guess about.
    // The device's own number, unscaled and unrounded. How many lines it is worth is the
    // core's answer, because it depends on a config key and a shell deciding it here would
    // be a second place that lives.
    guard isTypeable, event.scrollingDeltaY != 0 else { return }
    onScroll?(event.scrollingDeltaY > 0 ? "up" : "down", abs(event.scrollingDeltaY))
  }

  /// The clipboard, on its way to the pane.
  ///
  /// Reached through the responder chain from the Edit menu's ⌘V rather than by matching
  /// the chord in `keyDown`, because that is how macOS decides what ⌘V means - it honors a
  /// remapped shortcut, and it keeps working when the key equivalent is not the one we
  /// assumed.
  ///
  /// Not an override: `paste(_:)` is an action `NSResponder` dispatches by selector rather
  /// than a method `NSView` declares, so this declares it.
  @objc public func paste(_ sender: Any?) {
    guard let text = pasteboard.string(forType: .string) else {
      Core.debug("input.paste.empty", ["impact": "nothing was sent; the clipboard has no text"])
      return
    }
    Core.paste(text: text)
  }

  /// What is selected in this pane, on its way to the clipboard.
  ///
  /// Reached through the responder chain from the Edit menu, for the reason paste is: that is
  /// how macOS decides what ⌘C means, and it keeps working when somebody has rebound it.
  ///
  /// A pane with nothing selected copies nothing rather than clearing the clipboard, which is
  /// what every other terminal does and what anyone who mistyped the chord expects.
  @objc public func copy(_ sender: Any?) {
    guard let selected = surface?.selectedText, !selected.isEmpty else {
      Core.debug(
        "selection.empty",
        ["impact": "nothing was copied; the clipboard still holds whatever it held"])
      return
    }
    pasteboard.clearContents()
    pasteboard.setString(selected, forType: .string)
    Core.debug("selection.copied", ["bytes": String(selected.utf8.count)])
  }

  /// Greys out an Edit item that would do nothing.
  ///
  /// AppKit enables an item as soon as something in the responder chain implements it, so
  /// without this Copy looks available in a pane with nothing selected and then does nothing
  /// when pressed. A menu that lies about what it can do is the same failure as a window that
  /// lies about being typeable, one order of magnitude smaller.
  public func validateMenuItem(_ item: NSMenuItem) -> Bool {
    switch item.action {
    case #selector(copy(_:)):
      return surface?.selectedText?.isEmpty == false
    case #selector(paste(_:)):
      return pasteboard.string(forType: .string) != nil
    default:
      // Anything else in the chain answers for itself; a view that claimed on their behalf
      // would grey out items it knows nothing about.
      return true
    }
  }

  public override func becomeFirstResponder() -> Bool {
    surface?.setFocus(true)
    return true
  }

  public override func resignFirstResponder() -> Bool {
    surface?.setFocus(false)
    return true
  }
}

/// Input method support.
///
/// Without this, composing scripts are unusable: dead keys, pinyin, kana and every
/// candidate window need somewhere to put text that is not finished yet. AppKit routes all
/// of it through this protocol, and the only thing Muster does with it is refuse to send
/// anything until the method says it is done.
extension SurfaceView: @preconcurrency NSTextInputClient {
  public func insertText(_ string: Any, replacementRange: NSRange) {
    markedText = NSMutableAttributedString()
    let text =
      switch string {
      case let attributed as NSAttributedString: attributed.string
      case let plain as String: plain
      default: ""
      }
    guard !text.isEmpty else { return }
    guard interpretingKeyEvent else {
      // Not from a keystroke at all - a menu, a service, a character picker. Nothing else
      // is going to send this, so it goes now.
      Core.send(text: text)
      return
    }
    committedText = (committedText ?? "") + text
  }

  public func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
    switch string {
    case let attributed as NSAttributedString:
      markedText = NSMutableAttributedString(attributedString: attributed)
    case let plain as String: markedText = NSMutableAttributedString(string: plain)
    default: break
    }
  }

  public func unmarkText() {
    markedText = NSMutableAttributedString()
  }

  public func hasMarkedText() -> Bool {
    markedText.length > 0
  }

  public func markedRange() -> NSRange {
    markedText.length > 0
      ? NSRange(location: 0, length: markedText.length) : NSRange(location: NSNotFound, length: 0)
  }

  public func selectedRange() -> NSRange {
    NSRange(location: NSNotFound, length: 0)
  }

  public func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?)
    -> NSAttributedString?
  { nil }

  public func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }

  /// Where the input method should put its candidate window.
  ///
  /// The pane's cursor is daemon truth and the frame stream does not carry it, so this
  /// answers with the view's own origin. A candidate window in the wrong corner is a
  /// papercut; refusing to compose until the cursor is knowable would be worse.
  public func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
    guard let window else { return .zero }
    return window.convertToScreen(convert(bounds, to: nil))
  }

  public func characterIndex(for point: NSPoint) -> Int { 0 }
}
