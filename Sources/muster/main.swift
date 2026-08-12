import AppKit
import MusterRenderer

// The spike shell: one window, one surface, running $SHELL. Its whole job is to prove
// the embedding API drives from outside ghostty's own app, so it wires OS events in and
// nothing else. Real windows, splits, and keymap precedence come later, and belong in
// the core rather than here.

/// Hosts one libghostty surface. libghostty attaches its own Metal layer to whatever
/// NSView it is handed, so this view draws nothing itself.
@MainActor
final class SurfaceView: NSView {
  private var surface: Surface?

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

  func attach(_ surface: Surface) {
    self.surface = surface
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

  // Input here is deliberately the crudest thing that proves the surface is live:
  // committed text only. Muster's real input path does not run through the surface at
  // all - the frame stream consumes the pane's terminal modes, so the core encodes with
  // libghostty-vt and the daemon re-encodes. Building an NSEvent-to-ghostty key
  // translation now would be throwaway work.
  override func keyDown(with event: NSEvent) {
    guard let surface, let text = event.characters, !text.isEmpty else { return }
    surface.sendText(text)
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

/// What the surface should run: a herdr pane if one was named, otherwise a plain shell.
///
/// `muster w1:p1` mirrors a daemon-owned pane; bare `muster` runs $SHELL, which is the
/// spike's own terminal and useful for telling a renderer problem apart from a backend
/// one. Choosing panes properly is the core's job and does not belong on a command line.
func paneCommand() -> String? {
  guard CommandLine.arguments.count > 1 else {
    return ProcessInfo.processInfo.environment["SHELL"]
  }
  let paneID = CommandLine.arguments[1]
  let bridge = URL(fileURLWithPath: CommandLine.arguments[0])
    .deletingLastPathComponent()
    .appendingPathComponent("muster-bridge")
  return "\(bridge.path) \(paneID)"
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
  private var window: NSWindow?
  private var renderer: Renderer?

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

      view.attach(try renderer.makeSurface(in: view, command: paneCommand()))
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

  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
