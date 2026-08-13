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

  /// What the input method committed while the current `keyDown` was being interpreted.
  ///
  /// Held rather than sent, because at the moment `insertText` runs it is not yet known
  /// whether this is a composition finishing - which the pane should receive as text - or
  /// AppKit simply handing back the character that was typed, which the encoder is about
  /// to produce anyway. Sending from both places is how every keystroke arrived twice.
  private var committedText: String?

  /// Whether `insertText` is being called from inside `keyDown`.
  private var interpretingKeyEvent = false

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
    let wasComposing = hasMarkedText()
    committedText = nil
    interpretingKeyEvent = true
    interpretKeyEvents([event])
    interpretingKeyEvent = false

    // Still composing: this keystroke was the input method's, not the pane's.
    guard !hasMarkedText() else { return }

    // A composition that just finished is the one case where the committed text is the
    // truth and the keystroke is not: what an input method produces need not resemble the
    // key that produced it. Every other press goes to the encoder, which already carries
    // the typed characters on the event.
    if wasComposing, let text = committedText, !text.isEmpty {
      pane?.send(text: text)
      return
    }
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

  /// The clipboard, on its way to the pane.
  ///
  /// Reached through the responder chain from the Edit menu's ⌘V rather than by matching
  /// the chord in `keyDown`, because that is how macOS decides what ⌘V means - it honors
  /// a remapped shortcut, and it keeps working when the key equivalent is not the one we
  /// assumed. Muster's own keymap will take precedence over this later; the seam for that
  /// already exists in `PaneInput`.
  /// Not an override: `paste(_:)` is an action `NSResponder` dispatches by selector rather
  /// than a method `NSView` declares, so this declares it.
  @objc func paste(_ sender: Any?) {
    guard let text = NSPasteboard.general.string(forType: .string) else {
      Log.debug("input.paste.empty", ["impact": "nothing was sent; the clipboard has no text"])
      return
    }
    pane?.paste(text: text)
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
    guard interpretingKeyEvent else {
      // Not from a keystroke at all - a menu, a service, a character picker. Nothing else
      // is going to send this, so it goes now.
      pane?.send(text: text)
      return
    }
    // Committed text goes as text rather than as keystrokes: what the method produced may
    // have no relationship to the keys that produced it. keyDown decides whether that is
    // what happened here.
    committedText = (committedText ?? "") + text
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
  private let profile: TerminalModeProfile
  private var warnedAboutDroppedInput = false

  init(channel: PaneControlChannel, profile: TerminalModeProfile) throws {
    self.profile = profile
    self.channel = channel
    self.encoder = try KeyEncoder(profile: profile)
  }

  func send(_ key: KeyEvent) {
    // Precedence: the keymap gets first refusal, and the encoder only sees what it
    // declines.
    switch keymap.resolve(key) {
    case .text(let bytes):
      Log.debug(
        "input.bound.text",
        [
          "key": "\(key.key)", "mods": "\(key.modifiers.rawValue)", "bytes": String(bytes.count),
        ])
      deliver(.input(bytes))
      return
    case .action:
      Log.debug("input.bound", ["key": "\(key.key)", "mods": "\(key.modifiers.rawValue)"])
      return
    case .unbound:
      break
    }
    guard let bytes = try? encoder.encode(key) else {
      Log.warn(
        "input.encode.failed",
        [
          "key": "\(key.key)", "mods": "\(key.modifiers.rawValue)",
          "impact": "this keystroke reaches the pane as nothing at all",
        ])
      return
    }
    // An empty encoding is normal and frequent - modifiers alone, and every key while an
    // input method is composing - so it is not a warning, but a silence worth being able
    // to tell apart from a dropped one.
    guard !bytes.isEmpty else {
      Log.trace("input.key.empty", ["key": "\(key.key)", "action": "\(key.action)"])
      return
    }
    Log.debug(
      "input.key",
      [
        "key": "\(key.key)", "mods": "\(key.modifiers.rawValue)", "action": "\(key.action)",
        "bytes": String(bytes.count),
        "encoded": Log.includesInput ? String(decoding: bytes, as: UTF8.self).debugDescription : "",
      ])
    deliver(.input(bytes))
  }

  func send(text: String) {
    Log.debug(
      "input.text",
      [
        "characters": String(text.count),
        "text": Log.includesInput ? text.debugDescription : "",
      ])
    deliver(.input(Array(text.utf8)))
  }

  /// Sends the clipboard to the pane.
  ///
  /// Separate from typed text because a paste is a thing a program can be told about: a
  /// program that enabled DEC 2004 wants the text fenced by paste markers, so that it can
  /// tell "the user pasted this" from "the user typed this very fast". Agents use exactly
  /// that distinction, and shells use it to stop a multi-line paste from running as it
  /// arrives.
  ///
  /// The fence is currently never applied, because `unknownPane` cannot know the mode and
  /// markers sent to a program that never asked for them arrive as literal `[200~` on its
  /// input. Guessing wrong is worse here than guessing low. Card a_27DO80J34 fixes this
  /// properly: paste is the ideal first user of herdr's own `pane.send_input`, which
  /// encodes against the pane's real modes, because one socket connect per paste costs
  /// nothing - unlike one per keystroke, which is what makes that trade hard elsewhere.
  func paste(text: String) {
    guard !text.isEmpty else { return }
    Log.info(
      "input.paste",
      [
        "characters": String(text.count),
        "bracketed": String(profile.bracketedPaste),
        "text": Log.includesInput ? text.debugDescription : "",
      ])
    guard profile.bracketedPaste else {
      deliver(.input(Array(text.utf8)))
      return
    }
    deliver(.input(Array("\u{1b}[200~\(text)\u{1b}[201~".utf8)))
  }

  func scroll(direction: ControlStreamMessage.ScrollDirection, lines: UInt16) {
    deliver(.scroll(direction: direction, lines: lines))
  }

  private func deliver(_ message: ControlStreamMessage) {
    guard !channel.send(message) else { return }
    Log.warn(
      "input.dropped",
      [
        "socket": channel.socketPath,
        "impact": "the pane looks frozen but is fine; nothing typed here reached it",
      ])
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
    // First, so that everything after it is on the record - including the failures that
    // terminate this method.
    if let logPath = startLogging() {
      FileHandle.standardError.write(Data("muster: logging to \(logPath)\n".utf8))
    }
    Log.info(
      "app.launch",
      [
        "args": CommandLine.arguments.dropFirst().joined(separator: " "),
        "input_recorded": String(Log.includesInput),
      ])

    installMenu()
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
      let command = paneCommand(controlSocketPath: channel?.socketPath)
      Log.info("surface.create", ["command": command ?? "(none)"])
      view.attach(try renderer.makeSurface(in: view, command: command), pane: pane)
      renderer.setFocus(true)
      window.makeFirstResponder(view)
      Log.info("app.ready", ["typeable": String(pane != nil)])
    } catch {
      // A spike that fails silently teaches nothing. Say which step broke, because each
      // one fails for a different reason: init and app creation mean the embedding API
      // itself is not usable here, surface creation means the view binding is wrong.
      Log.error("app.setup.failed", ["error": "\(error)"])
      FileHandle.standardError.write(Data("muster: renderer setup failed: \(error)\n".utf8))
      NSApp.terminate(nil)
    }
  }

  /// The input path for the pane this window shows, or nil when running a plain shell.
  ///
  /// A bare `muster` has no daemon behind it, and Muster's input path only knows how to
  /// talk to one: it encodes a keystroke and hands the bytes to a pane's control stream.
  /// A local shell has no control stream, so there is nowhere to put them.
  ///
  /// That makes bare `muster` an output-only check on the renderer, and it says so rather
  /// than presenting a terminal that ignores the keyboard. Wiring this mode up would mean
  /// calling `ghostty_surface_key` and putting input back inside the renderer seam
  /// (architecture.md), for a mode that ships in no version of Muster - every real pane
  /// comes from a daemon.
  private func makePaneInput() throws -> PaneInput? {
    guard CommandLine.arguments.count > 1 else {
      window?.title = "muster (renderer check - keyboard not connected)"
      FileHandle.standardError.write(
        Data(
          """
          muster: no pane named, so this window only proves the renderer works.
          It runs $SHELL and paints what that prints, but every keystroke is dropped - \
          input needs a daemon-owned pane to encode for. To type into one, pass its id: \
          `muster w1:p1`. `herdr pane list` names the panes that exist.

          """.utf8))
      return nil
    }

    window?.title = "muster - \(CommandLine.arguments[1])"
    let path = FileManager.default.temporaryDirectory
      .appendingPathComponent("muster-\(getpid()).sock").path
    let channel = try PaneControlChannel(path: path)
    self.channel = channel
    // The pane's modes are not readable, so this is the documented guess. One day it is
    // fed from the daemon; nothing above here changes when it is.
    return try PaneInput(channel: channel, profile: .unknownPane)
  }

  /// The smallest menu bar that makes the platform's shortcuts work.
  ///
  /// Not decoration: on macOS a key equivalent is dispatched from the main menu, so
  /// without this ⌘V and ⌘Q are inert no matter what the view implements. An app with no
  /// menu at all is also one a person cannot quit normally.
  ///
  /// Copy is deliberately absent - it needs a selection, and the pane's selection lives in
  /// the daemon where Muster cannot yet reach it. A menu item that silently does nothing
  /// would be worse than its absence.
  private func installMenu() {
    let menu = NSMenu()

    let appItem = NSMenuItem()
    let appMenu = NSMenu()
    appMenu.addItem(
      withTitle: "Quit muster", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
    appItem.submenu = appMenu
    menu.addItem(appItem)

    let editItem = NSMenuItem()
    let editMenu = NSMenu(title: "Edit")
    // nil target: AppKit walks the responder chain, which lands on the focused surface.
    editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
    editItem.submenu = editMenu
    menu.addItem(editItem)

    NSApp.mainMenu = menu
  }

  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
