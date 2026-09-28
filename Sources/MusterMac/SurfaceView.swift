import AppKit
import MusterRenderer

/// Hosts one libghostty surface, and feeds the pane it mirrors.
///
/// libghostty attaches its own Metal layer to whatever NSView it is handed, so this view
/// draws nothing itself. What it does own is input: the surface is a renderer, and nothing
/// typed here reaches the pane through it (architecture.md, the renderer seam).
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

  /// Whether the window holding this pane is key, and whether it can be seen, as the window last
  /// said. Kept here so that a surface attached later starts from the window's answer rather
  /// than from libghostty's, which is focused and visible.
  private var windowIsKey = false
  private var windowIsVisible = true

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
  public var onClick: (@MainActor () -> Void)?

  /// Called when the wheel turns, a button goes down or up, or the pointer moves over this view,
  /// after the surface has been handed the same event.
  ///
  /// Reported rather than sent, for the same reason a click is: the view under the pointer
  /// knows the gesture happened and nothing else, and which pane that is belongs to the chrome
  /// around it. AppKit hit-tests pointer events to the view the pointer is over, so this fires
  /// on the right surface whether or not it is the one with the keyboard - which is what lets
  /// somebody read one agent while typing into another.
  public var onPointer: (@MainActor (Core.Pointer) -> Void)?

  /// Called when this pane's search has counted its matches or moved to one. Held here rather
  /// than on the surface, because the find bar can open before the pane's surface exists.
  public var onSearch: (@MainActor (SearchReport) -> Void)?

  public override init(frame: NSRect) {
    super.init(frame: frame)
    // Layer-backed before the surface is created, and on the main thread. libghostty's
    // renderer runs on its own thread and wants a layer waiting for it; making it ask
    // AppKit for one from there trips a dispatch-queue assertion. ghostty's own app never
    // hits this because SwiftUI has already made its view hierarchy layer-backed.
    wantsLayer = true
    updateTrackingAreas()
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
    if !windowIsVisible {
      surface.setOcclusion(visible: false)
    }
    surface.onProcessExited = { [weak self] processAlive in
      self?.paneEnded(processAlive: processAlive)
    }
    surface.onSearch = { [weak self] report in
      self?.onSearch?(report)
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

  /// Searches this pane for a needle, and `nil` ends the search.
  ///
  /// Silently nothing before a surface is attached, on the same terms as sizing the text: a
  /// pane whose bridge has not started has nothing to search, and refusing would be reporting a
  /// renderer problem for a pane with no renderer yet.
  @discardableResult
  public func search(_ needle: String?) -> [String] {
    surface?.search(needle) ?? []
  }

  /// Moves to the next match of this pane's search, or the previous one.
  public func navigateSearch(next: Bool) {
    surface?.navigateSearch(next: next)
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
    // Once for the whole keystroke rather than on every callback inside it, and only while a
    // composition is or was showing: clearing one that never was would redraw the pane on
    // every key.
    if wasComposing || hasMarkedText() { showComposition() }

    guard isTypeable else { return }
    // All three signals travel together and the core picks between them. Choosing here
    // would be the shell deciding what a keystroke means, and choosing *both* - the
    // committed text and the encoded key - is the bug that made `hello` arrive as
    // `hheelllloo`.
    let toPane = Core.send(
      keyDown: event.musterKeyEvent(
        action: event.isARepeat ? "repeated" : "press", isComposing: hasMarkedText()),
      wasComposing: wasComposing,
      committed: committedText,
      stillComposing: hasMarkedText())
    // Only keys the program got (SurfaceKeys.swift says why the surface gets them at all), so
    // a chord Muster kept moves nothing in the pane.
    guard toPane else { return }
    surface?.pressKey(
      event, committed: committedText, composing: wasComposing || hasMarkedText())
  }

  public override func keyUp(with event: NSEvent) {
    // Only reported when the pane asked for release events, which the encoder decides from
    // the mode profile. Sending it unconditionally is how a program that never asked ends
    // up with every keystroke twice.
    guard isTypeable else { return }
    Core.send(
      keyUp: event.musterKeyEvent(
        action: event.isARepeat ? "repeated" : "release", isComposing: hasMarkedText()))
    surface?.releaseKey(event)
  }

  public override func flagsChanged(with event: NSEvent) {
    // A modifier held changes what the surface does with the pointer, as it does in Ghostty.
    // Not mid-composition, where the modifiers belong to the input method.
    if isTypeable, !hasMarkedText() {
      surface?.changeModifiers(event)
    }
    // Onward regardless: the window watches modifiers for the numbered chords, and an override
    // here that stopped the event would end those silently.
    super.flagsChanged(with: event)
  }

  /// A click into a window that is not key still picks the pane, rather than being spent
  /// activating the app. Fifteen panes make the alternative - click once to focus the window,
  /// again to pick the pane - a papercut on every switch back.
  public override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  // Every button and every movement goes to the surface and to the pane's daemon, and neither
  // is decided here.
  //
  // The surface sees the pane's real byte stream, so it knows the program's modes: a drag over
  // a program that asked for the mouse is a report rather than a selection, and what the
  // surface writes for it is dropped by the bridge. The program gets its report from the
  // daemon, which holds the same modes and applies them - shift included, so a shift-drag
  // selects even there, unless the program asked to be sent shift-clicks (XTSHIFTESCAPE),
  // which both read from the same bytes.

  public override func mouseDown(with event: NSEvent) {
    onClick?()
    button(event, 0, pressed: true)
  }

  public override func mouseUp(with event: NSEvent) {
    button(event, 0, pressed: false)
  }

  public override func rightMouseDown(with event: NSEvent) {
    // Not consumed is libghostty leaving the click to the host, which is AppKit's context menu
    // - none today, and whatever `menu(for:)` answers if that changes.
    if !button(event, 1, pressed: true) { super.rightMouseDown(with: event) }
  }

  public override func rightMouseUp(with event: NSEvent) {
    if !button(event, 1, pressed: false) { super.rightMouseUp(with: event) }
  }

  public override func otherMouseDown(with event: NSEvent) {
    button(event, event.buttonNumber, pressed: true)
  }

  public override func otherMouseUp(with event: NSEvent) {
    button(event, event.buttonNumber, pressed: false)
  }

  public override func mouseMoved(with event: NSEvent) { moved(event) }
  public override func mouseDragged(with event: NSEvent) { moved(event) }
  public override func rightMouseDragged(with event: NSEvent) { moved(event) }
  public override func otherMouseDragged(with event: NSEvent) { moved(event) }

  /// Asks for `mouseMoved` with no button held, which AppKit sends only to a view that tracks
  /// it. Ghostty's options: the visible rect, so a pane half behind the sidebar is not reported
  /// through it, and always, because a program asking for motion gets it whether or not this
  /// window is key.
  public override func updateTrackingAreas() {
    trackingAreas.forEach(removeTrackingArea)
    addTrackingArea(
      NSTrackingArea(
        rect: bounds, options: [.mouseMoved, .inVisibleRect, .activeAlways], owner: self))
    super.updateTrackingAreas()
  }

  /// Hands a button to the surface and then to the pane's daemon, and says whether the surface
  /// consumed it.
  @discardableResult
  private func button(_ event: NSEvent, _ number: Int, pressed: Bool) -> Bool {
    // The position first, because a button event is about wherever the pointer already is and
    // libghostty holds that separately - a press reported without one starts the selection at
    // the last place the pointer was seen.
    surface?.mouseMoved(to: flipped(event), modifiers: event.modifierFlags)
    let consumed =
      surface?.mouseButton(number, pressed: pressed, modifiers: event.modifierFlags) ?? false
    report(event, pressed ? .press : .release)
    return consumed
  }

  private func moved(_ event: NSEvent) {
    surface?.mouseMoved(to: flipped(event), modifiers: event.modifierFlags)
    report(event, .motion)
  }

  private func report(_ event: NSEvent, _ action: Core.Mouse.Action) {
    guard isTypeable, let button = Self.reportedButton(event) else { return }
    let (x, y) = pixels(event)
    onPointer?(
      .mouse(
        Core.Mouse(
          action: action, button: button, modifiers: event.modifierFlags.musterNames, x: x, y: y)
      ))
  }

  /// The button a pane's daemon is told about: the one pressed or released, or for a drag the
  /// one held.
  ///
  /// Nil for the buttons past the middle one. A terminal reports only three, and a back button
  /// sent as "none" would be a lie a program could act on - motion while a button is held
  /// reported as motion with nothing held.
  private static func reportedButton(_ event: NSEvent) -> Core.Mouse.Button? {
    switch event.type {
    // Spelled out, because a bare `.none` here is the optional's nil.
    case .mouseMoved: Core.Mouse.Button.none
    case .leftMouseDown, .leftMouseUp, .leftMouseDragged: .left
    case .rightMouseDown, .rightMouseUp, .rightMouseDragged: .right
    default: event.buttonNumber == 2 ? .middle : nil
    }
  }

  /// Where an event landed, measured from this view's top left.
  ///
  /// AppKit measures this view from the bottom left and the surface measures itself from the
  /// top left, so an unflipped position selects the mirror image of the drag.
  private func flipped(_ event: NSEvent) -> NSPoint {
    let point = convert(event.locationInWindow, from: nil)
    return NSPoint(x: point.x, y: frame.height - point.y)
  }

  /// Where an event landed in the pixels a pane's daemon measures its terminal in: backing
  /// pixels, from the top left of its text.
  ///
  /// From the text rather than the view's edge, because the terminal's size is what its cells
  /// cover, as libghostty sizes it. Ghostty counts a position over the padding as the nearest
  /// cell and one off the surface as outside it, and the daemon's encoder knows only the
  /// cells, so a position over the padding is moved onto them here and one off the view is
  /// left for the encoder's own rules about the world outside a terminal.
  private func pixels(_ event: NSEvent) -> (x: Double, y: Double) {
    let scale = window?.backingScaleFactor ?? 2
    let at = flipped(event)
    let inset = surface?.padding ?? 0
    var x = Double((at.x - inset) * scale)
    var y = Double((at.y - inset) * scale)
    if bounds.contains(convert(event.locationInWindow, from: nil)),
      let text = surface?.textPixelSize
    {
      x = min(max(0, x), Double(text.width) - 1)
      y = min(max(0, y), Double(text.height) - 1)
    }
    return (x, y)
  }

  public override func scrollWheel(with event: NSEvent) {
    // Built as Ghostty's own view builds it, precise deltas doubled included, so a pane scrolls
    // as far here as it would there.
    let precise = event.hasPreciseScrollingDeltas
    let scale = precise ? 2.0 : 1.0
    let dx = Double(event.scrollingDeltaX) * scale
    let dy = Double(event.scrollingDeltaY) * scale
    let momentum = scrollMomentum(event.momentumPhase)
    surface?.scroll(dx: dx, dy: dy, precise: precise, momentum: momentum)

    // The surface has scrolled its own history, or answered as the pane's modes say, and what
    // it wrote back is dropped. The program in the pane is owed the same gesture, and only its
    // daemon may write to it. The renderer check has no daemon to tell.
    guard isTypeable else { return }
    let (x, y) = pixels(event)
    onPointer?(
      .wheel(
        Core.Wheel(
          dx: dx, dy: dy, precise: precise, momentum: momentum,
          modifiers: event.modifierFlags.musterNames, x: x, y: y)))
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

  /// Carries out one of Ghostty's surface-local binding actions on this pane.
  public func perform(_ action: SurfaceAction) {
    guard let refused = surface?.perform(action), !refused.isEmpty else { return }
    Core.debug(
      "surface.action.not_performed",
      [
        "action": refused.joined(separator: ","),
        "impact": "nothing moved; ordinary when there was nowhere to move to, such as no prompt "
          + "above the first",
        "check": "if it never works, whether a libghostty re-pin renamed the action",
      ])
  }

  /// A surface is focused only while it has the keyboard in a key window, as in Ghostty. The
  /// focused one's display link fires every frame, which is a cost for a window nobody is using.
  public func apply(windowIsKey key: Bool) {
    windowIsKey = key
    surface?.setFocus(key && window?.firstResponder === self)
  }

  /// libghostty stops drawing a surface only when told it cannot be seen.
  public func apply(windowIsVisible visible: Bool) {
    guard visible != windowIsVisible else { return }
    windowIsVisible = visible
    surface?.setOcclusion(visible: visible)
  }

  public override func becomeFirstResponder() -> Bool {
    surface?.setFocus(windowIsKey)
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
/// of it through this protocol. Muster sends nothing until the method says it is done, and
/// meanwhile the surface draws the composition at the pane's cursor, where the candidate
/// window opens beside it, as in Ghostty.
extension SurfaceView: @preconcurrency NSTextInputClient {
  public func insertText(_ string: Any, replacementRange: NSRange) {
    let wasComposing = hasMarkedText()
    markedText = NSMutableAttributedString()
    if wasComposing, !interpretingKeyEvent { showComposition() }
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
    // Inside a keystroke `keyDown` shows it once the keystroke is done. Outside one - a layout
    // switched mid-composition - nothing else will.
    if !interpretingKeyEvent { showComposition() }
  }

  public func unmarkText() {
    guard hasMarkedText() else { return }
    markedText = NSMutableAttributedString()
    if !interpretingKeyEvent { showComposition() }
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

  /// Where the input method should put its candidate window: on the pane's cursor, so it
  /// opens beside the text being composed. The view itself before there is a surface to ask.
  public func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
    guard let window else { return .zero }
    guard let surface else { return window.convertToScreen(convert(bounds, to: nil)) }
    let scale = window.backingScaleFactor
    let cellWidth = Double(surface.cellPixelSize?.width ?? 0) / scale
    let rect = Self.candidateRect(
      cursor: surface.cursorCell, range: range, cellWidth: cellWidth, height: bounds.height)
    return window.convertToScreen(convert(rect, to: nil))
  }

  public func characterIndex(for point: NSPoint) -> Int { 0 }

  /// The cursor's cell, as the surface measures it from its top left, in this view's own
  /// coordinates from the bottom left.
  ///
  /// An empty range is an insertion point rather than text - dictation asks with one to place
  /// its microphone - so it is a line at the range's position rather than a cell, as Ghostty
  /// answers it.
  static func candidateRect(cursor: NSRect, range: NSRange, cellWidth: Double, height: Double)
    -> NSRect
  {
    var x = cursor.minX
    var width = cursor.width
    if range.length == 0, width > 0 {
      width = 0
      x += cellWidth * Double(range.location)
    }
    return NSRect(x: x, y: height - cursor.minY, width: width, height: cursor.height)
  }

  /// Hands the composition to the surface to draw, or clears it when there is none.
  private func showComposition() {
    surface?.setPreedit(hasMarkedText() ? markedText.string : nil)
  }
}
