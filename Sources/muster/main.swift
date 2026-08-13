import AppKit
import MusterCore
import MusterHerdr
import MusterMac
import MusterRenderer
import MusterVT

// The spike shell: one window, one surface, one pane. Its whole job is to stand things up
// and hand them to each other. Nothing here decides anything - which chord is an action,
// what bytes a keystroke becomes, whether a composition finished - because none of that
// can be reached by a test from an executable target, and all of it has been wrong at
// least once (docs/testing.md: if something is hard to test, it is in the wrong layer).
//
// Real windows, splits and a configurable keymap come later, and belong in the core.

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
  private var window: NSWindow?
  private var renderer: Renderer?
  private var channel: PaneControlChannel?

  func applicationDidFinishLaunching(_ notification: Notification) {
    // First, so that everything after it is on the record - including the failures that
    // terminate this method.
    let logPath = startLogging()
    if let logPath {
      FileHandle.standardError.write(Data("muster: logging to \(logPath)\n".utf8))
    }
    // The core owns the file from here. The shell decided where it goes, which is the one
    // part of logging that is an OS question (architecture.md, the diagnostic log).
    Core.start(logPath: logPath)
    Core.info(
      "app.launch",
      [
        "args": CommandLine.arguments.dropFirst().joined(separator: " "),
        "input_recorded": String(Core.includesInput),
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
      Core.info("surface.create", ["command": command ?? "(none)"])
      view.attach(try renderer.makeSurface(in: view, command: command), pane: pane)
      renderer.setFocus(true)
      window.makeFirstResponder(view)
      Core.info("app.ready", ["typeable": String(pane != nil)])
    } catch {
      // A spike that fails silently teaches nothing. Say which step broke, because each
      // one fails for a different reason: init and app creation mean the embedding API
      // itself is not usable here, surface creation means the view binding is wrong.
      Core.error("app.setup.failed", ["error": "\(error)"])
      FileHandle.standardError.write(Data("muster: renderer setup failed: \(error)\n".utf8))
      NSApp.terminate(nil)
    }
  }

  /// The input path for the pane this window shows, or nil when running a plain shell.
  ///
  /// A bare `muster` has no daemon behind it, and Muster's input path only knows how to
  /// talk to one: it encodes a keystroke and hands the bytes to a pane's control stream. A
  /// local shell has no control stream, so there is nowhere to put them.
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

    let paneID = CommandLine.arguments[1]
    window?.title = "muster - \(paneID)"
    let path = FileManager.default.temporaryDirectory
      .appendingPathComponent("muster-\(getpid()).sock").path
    let channel = try PaneControlChannel(path: path)
    self.channel = channel

    // The second channel, for the keys and text whose correct encoding depends on modes
    // the control stream cannot show us. Optional on purpose: no daemon socket means the
    // pane still works, with a guess.
    let server = HerdrPaneChannel(paneID: paneID)
    if server == nil {
      Core.warn(
        "app.server_channel.unavailable",
        [
          "impact": "arrow keys and paste fall back to a guessed encoding, "
            + "which pagers reject and multi-line pastes run as commands"
        ])
    }

    // The pane's modes are not readable, so this is the documented guess. One day it is
    // fed from the daemon; nothing above here changes when it is.
    return PaneInput(
      channel: channel, serverChannel: server, encoder: try KeyEncoder(profile: .unknownPane))
  }

  /// The smallest menu bar that makes the platform's shortcuts work.
  ///
  /// Not decoration: on macOS a key equivalent is dispatched from the main menu, so without
  /// this ⌘V and ⌘Q are inert no matter what the view implements. An app with no menu at
  /// all is also one a person cannot quit normally.
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

/// What the surface should run: a herdr pane if one was named, otherwise a plain shell.
///
/// `muster w1:p1` mirrors a daemon-owned pane; bare `muster` runs $SHELL, which is the
/// spike's own terminal and useful for telling a renderer problem apart from a backend one.
/// Choosing panes properly is the core's job and does not belong on a command line.
func paneCommand(controlSocketPath: String?) -> String? {
  guard CommandLine.arguments.count > 1 else {
    return ProcessInfo.processInfo.environment["SHELL"]
  }
  return PaneCommand.bridge(
    executable: CommandLine.arguments[0],
    paneID: CommandLine.arguments[1],
    controlSocketPath: controlSocketPath)
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
