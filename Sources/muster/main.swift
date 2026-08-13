import AppKit
import MusterCore
import MusterHerdr
import MusterRenderer
import MusterVT

// The spike shell: one window, one surface, one pane. Its whole job is to wire OS events
// into the core and render what comes back, so everything decidable - which chord is an
// action, what bytes a keystroke becomes - lives below this file. Real windows, splits,
// and a configurable keymap come later, and belong in the core rather than here.

/// Hosts one libghostty surface, and feeds the pane it mirrors.
///
/// libghostty attaches its own Metal layer to whatever NSView it is handed, so this view
/// draws nothing itself. What it does own is input: the surface is a renderer, and
/// nothing typed here goes through it (architecture.md, the renderer seam).
@MainActor
final class SurfaceView: NSView {
  private var surface: Surface?
  private var pane: PaneInput?

  /// Text the input method has committed but not yet handed over, and the marked range
  /// that says composition is still in progress.
  private var markedText = NSMutableAttributedString()

  override init(frame: NSRect) {
    super.init(frame: frame)
    // Layer-backed before the surface is created, and on the main thread. libghostty's
    // renderer runs on its own thread and wants a layer waiting for it; making it ask
    // AppKit for one from there trips a dispatch-queue assertion. ghostty's own app
    // never hits this because SwiftUI has already made its view hierarchy layer-backed.
    wantsLayer = true
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  override var acceptsFirstResponder: Bool { true }

  func attach(_ surface: Surface, pane: PaneInput?) {
    self.surface = surface
    self.pane = pane
    surface.setSize(
      width: UInt32(bounds.width * (window?.backingScaleFactor ?? 2)),
      height: UInt32(bounds.height * (window?.backingScaleFactor ?? 2)))
  }

  override func setFrameSize(_ newSize: NSSize) {
    super.setFrameSize(newSize)
    let scale = window?.backingScaleFactor ?? 2
    surface?.setSize(width: UInt32(newSize.width * scale), height: UInt32(newSize.height * scale))
  }

  // No draw override. libghostty runs its own display link on its own renderer thread
  // and paints the layer it attached to this view, so a host that also calls
  // ghostty_surface_draw is a second painter racing the first.

  override func keyDown(with event: NSEvent) {
    // The input method gets the event first. While it is composing, the keystrokes
    // belong to it - they select candidates and build characters - and the pane must see
    // only what it commits. interpretKeyEvents calls back into insertText or
    // setMarkedText below.
    interpretKeyEvents([event])
    guard !hasMarkedText() else { return }
    send(event, action: .press)
  }

  override func keyUp(with event: NSEvent) {
    // Only reported when the pane asked for release events, which the encoder decides
    // from the mode profile. Sending it unconditionally is how a program that never
    // asked ends up with every keystroke twice.
    send(event, action: .release)
  }

  private func send(_ event: NSEvent, action: KeyEvent.Action) {
    guard let pane,
      let key = event.musterKeyEvent(
        action: event.isARepeat ? .repeated : action, isComposing: hasMarkedText())
    else { return }
    pane.send(key)
  }

  override func scrollWheel(with event: NSEvent) {
    // Scroll never becomes bytes here. It goes out as an intent, and the daemon answers
    // it against the pane's real modes - the one input-shaped thing Muster does not have
    // to guess about.
    guard let pane, event.scrollingDeltaY != 0 else { return }
    let lines = max(1, UInt16(abs(event.scrollingDeltaY).rounded()))
    pane.scroll(direction: event.scrollingDeltaY > 0 ? .up : .down, lines: lines)
  }

  override func becomeFirstResponder() -> Bool {
    surface?.setFocus(true)
    return true
  }

  override func resignFirstResponder() -> Bool {
    surface?.setFocus(false)
    return true
  }
}

/// Input method support.
///
/// Without this, composing scripts are unusable: dead keys, pinyin, kana and every
/// candidate window need somewhere to put text that is not finished yet. AppKit routes
/// all of it through this protocol, and the only thing Muster does with it is refuse to
/// send anything until the method says it is done.
extension SurfaceView: @preconcurrency NSTextInputClient {
  func insertText(_ string: Any, replacementRange: NSRange) {
    markedText = NSMutableAttributedString()
    let text =
      switch string {
      case let attributed as NSAttributedString: attributed.string
      case let plain as String: plain
      default: ""
      }
    guard !text.isEmpty else { return }
    // Committed text goes as text rather than as keystrokes: what the method produced
    // may have no relationship to the keys that produced it.
    pane?.send(text: text)
  }

  func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
    switch string {
    case let attributed as NSAttributedString:
      markedText = NSMutableAttributedString(attributedString: attributed)
    case let plain as String: markedText = NSMutableAttributedString(string: plain)
    default: break
    }
  }

  func unmarkText() {
    markedText = NSMutableAttributedString()
  }

  func hasMarkedText() -> Bool {
    markedText.length > 0
  }

  func markedRange() -> NSRange {
    markedText.length > 0
      ? NSRange(location: 0, length: markedText.length) : NSRange(location: NSNotFound, length: 0)
  }

  func selectedRange() -> NSRange {
    NSRange(location: NSNotFound, length: 0)
  }

  func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?)
    -> NSAttributedString?
  { nil }

  func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }

  /// Where the input method should put its candidate window.
  ///
  /// The pane's cursor is daemon truth and the frame stream does not carry it, so this
  /// answers with the view's own origin. A candidate window in the wrong corner is a
  /// papercut; refusing to composeUntil the cursor is knowable would be worse.
  func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
    guard let window else { return .zero }
    return window.convertToScreen(convert(bounds, to: nil))
  }

  func characterIndex(for point: NSPoint) -> Int { 0 }
}

/// One pane's input path: keymap first, then encode, then out to the daemon.
///
/// The whole of "what happens when you type" in one place, so the shell above it only
/// has to decide *that* a key was pressed and this decides what it means.
@MainActor
final class PaneInput {
  private let channel: PaneControlChannel
  private let encoder: KeyEncoder
  private let keymap = Keymap()
  private var warnedAboutDroppedInput = false

  init(channel: PaneControlChannel, profile: TerminalModeProfile) throws {
    self.channel = channel
    self.encoder = try KeyEncoder(profile: profile)
  }

  func send(_ key: KeyEvent) {
    // Precedence: a bound chord is Muster's and the pane never sees it.
    guard case .unbound = keymap.resolve(key) else { return }
    guard let bytes = try? encoder.encode(key), !bytes.isEmpty else { return }
    deliver(.input(bytes))
  }

  func send(text: String) {
    deliver(.input(Array(text.utf8)))
  }

  func scroll(direction: ControlStreamMessage.ScrollDirection, lines: UInt16) {
    deliver(.scroll(direction: direction, lines: lines))
  }

  private func deliver(_ message: ControlStreamMessage) {
    guard !channel.send(message) else { return }
    // Once, not per keystroke: a pane that swallows input produces a lot of them, and a
    // log that scrolls is a log nobody reads.
    guard !warnedAboutDroppedInput else { return }
    warnedAboutDroppedInput = true
    FileHandle.standardError.write(
      Data(
        """
        muster: the pane bridge is not connected, so input is going nowhere.
        The pane keeps rendering, which makes this look like a frozen program rather \
        than a broken channel. Usual causes: muster-bridge failed to start (its own \
        error is above), or it could not reach \(channel.socketPath).

        """.utf8))
  }
}

/// What the surface should run: a herdr pane if one was named, otherwise a plain shell.
///
/// `muster w1:p1` mirrors a daemon-owned pane; bare `muster` runs $SHELL, which is the
/// spike's own terminal and useful for telling a renderer problem apart from a backend
/// one. Choosing panes properly is the core's job and does not belong on a command line.
func paneCommand(controlSocketPath: String?) -> String? {
  guard CommandLine.arguments.count > 1 else {
    return ProcessInfo.processInfo.environment["SHELL"]
  }
  let paneID = CommandLine.arguments[1]
  let bridge = URL(fileURLWithPath: CommandLine.arguments[0])
    .deletingLastPathComponent()
    .appendingPathComponent("muster-bridge")
  guard let controlSocketPath else { return "\(bridge.path) \(paneID)" }
  return "\(bridge.path) \(paneID) --control-socket \(controlSocketPath)"
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
  private var window: NSWindow?
  private var renderer: Renderer?
  private var channel: PaneControlChannel?

  func applicationDidFinishLaunching(_ notification: Notification) {
    let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 960, height: 600))

    let window = NSWindow(
      contentRect: view.frame,
      styleMask: [.titled, .closable, .resizable, .miniaturizable],
      backing: .buffered,
      defer: false)
    window.title = "muster (spike)"
    window.contentView = view
    window.center()
    window.makeKeyAndOrderFront(nil)
    self.window = window

    do {
      let renderer = try Renderer()
      Renderer.current = renderer
      self.renderer = renderer

      // Bound before the surface exists, so the bridge cannot dial a socket that is not
      // listening yet.
      let pane = try makePaneInput()
      view.attach(
        try renderer.makeSurface(
          in: view, command: paneCommand(controlSocketPath: channel?.socketPath)),
        pane: pane)
      renderer.setFocus(true)
      window.makeFirstResponder(view)
    } catch {
      // A spike that fails silently teaches nothing. Say which step broke, because each
      // one fails for a different reason: init and app creation mean the embedding API
      // itself is not usable here, surface creation means the view binding is wrong.
      FileHandle.standardError.write(Data("muster: renderer setup failed: \(error)\n".utf8))
      NSApp.terminate(nil)
    }
  }

  /// The input path for the pane this window shows, or nil when running a plain shell.
  ///
  /// A bare `muster` has no daemon behind it, so the surface's own command owns its
  /// input and there is nothing for Muster to encode.
  private func makePaneInput() throws -> PaneInput? {
    guard CommandLine.arguments.count > 1 else { return nil }

    let path = FileManager.default.temporaryDirectory
      .appendingPathComponent("muster-\(getpid()).sock").path
    let channel = try PaneControlChannel(path: path)
    self.channel = channel
    // The pane's modes are not readable, so this is the documented guess. One day it is
    // fed from the daemon; nothing above here changes when it is.
    return try PaneInput(channel: channel, profile: .unknownPane)
  }

  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
