import AppKit
import MusterCore
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
public final class SurfaceView: NSView {
  private var surface: Surface?
  private var pane: PaneInput?

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

  public func attach(_ surface: Surface, pane: PaneInput?) {
    self.surface = surface
    self.pane = pane
    surface.setSize(
      width: UInt32(bounds.width * (window?.backingScaleFactor ?? 2)),
      height: UInt32(bounds.height * (window?.backingScaleFactor ?? 2)))
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

    switch CompositionArbiter.outcome(
      wasComposing: wasComposing, committed: committedText, stillComposing: hasMarkedText())
    {
    case .sendNothing:
      return
    case .sendText(let text):
      pane?.send(text: text)
    case .sendKey:
      send(event, action: .press)
    }
  }

  public override func keyUp(with event: NSEvent) {
    // Only reported when the pane asked for release events, which the encoder decides from
    // the mode profile. Sending it unconditionally is how a program that never asked ends
    // up with every keystroke twice.
    send(event, action: .release)
  }

  private func send(_ event: NSEvent, action: KeyEvent.Action) {
    guard let pane,
      let key = event.musterKeyEvent(
        action: event.isARepeat ? .repeated : action, isComposing: hasMarkedText())
    else { return }
    pane.send(key)
  }

  public override func scrollWheel(with event: NSEvent) {
    // Scroll never becomes bytes here. It goes out as an intent, and the daemon answers it
    // against the pane's real modes - the one input-shaped thing Muster does not have to
    // guess about.
    guard let pane, event.scrollingDeltaY != 0 else { return }
    let lines = max(1, UInt16(abs(event.scrollingDeltaY).rounded()))
    pane.scroll(direction: event.scrollingDeltaY > 0 ? .up : .down, lines: lines)
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
    guard let text = NSPasteboard.general.string(forType: .string) else {
      Log.debug("input.paste.empty", ["impact": "nothing was sent; the clipboard has no text"])
      return
    }
    pane?.paste(text: text)
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
      pane?.send(text: text)
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
