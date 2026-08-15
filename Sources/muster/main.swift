import AppKit
import MusterMac
import MusterRenderer

// The entry point, and nothing else. Its whole job is to stand things up and hand them to
// each other: nothing here decides anything - which chord is an action, what bytes a
// keystroke becomes, where a pane goes on screen - because none of that can be reached by a
// test from an executable target, and all of it has been wrong at least once (docs/testing.md:
// if something is hard to test, it is in the wrong layer).

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
  private var muster: MusterWindow?
  private var renderer: Renderer?

  func applicationDidFinishLaunching(_ notification: Notification) {
    // First, so that everything after it is on the record - including the failures that
    // terminate this method.
    let logPath = startLogging()
    if let logPath {
      FileHandle.standardError.write(Data("muster: logging to \(logPath)\n".utf8))
    }
    // The core owns the file from here. The shell decided where it goes, which is the one
    // part of logging that is an OS question (architecture.md, the diagnostic log).
    // The config file goes over at the same moment and for the same reason: where it lives
    // is an OS question, and what it says is the core's.
    let config = configPath()
    Core.start(logPath: logPath, configPath: config)
    Core.info(
      "app.launch",
      [
        "args": CommandLine.arguments.dropFirst().joined(separator: " "),
        "config": config ?? "(none)",
        "input_recorded": String(Core.includesInput),
      ])

    do {
      let renderer = try Renderer()
      Renderer.current = renderer
      self.renderer = renderer

      let muster = MusterWindow(renderer: renderer, executable: CommandLine.arguments[0])
      Core.window = muster
      self.muster = muster
      NSApp.mainMenu = AppMenu.build(target: muster)
      muster.show()

      // Everything about what this window shows is behind this call: the core finds the
      // daemon, opens a socket per pane, and publishes the whole view back - which is what
      // builds the surfaces. A window with no pane named renders the user's shell instead.
      let attached: Bool
      if let paneID = CommandLine.arguments.dropFirst().first {
        attached = Core.attach(paneID: paneID)
        if !attached {
          muster.report(problem: "\(paneID) could not be attached (see stderr)")
        }
      } else {
        explainRendererCheck()
        muster.showRendererCheck()
        attached = false
      }
      renderer.setFocus(true)
      Core.info("app.ready", ["typeable": String(attached)])
    } catch {
      // A failure here means the embedding API itself is not usable, so there is no window to
      // report into. Say which step broke on the way out.
      Core.error("app.setup.failed", ["error": "\(error)"])
      FileHandle.standardError.write(Data("muster: renderer setup failed: \(error)\n".utf8))
      NSApp.terminate(nil)
    }
  }

  /// A bare `muster` has no daemon behind it, and Muster's input path only knows how to talk
  /// to one: it encodes a keystroke and hands the bytes to a pane's control stream. A local
  /// shell has no control stream, so there is nowhere to put them.
  ///
  /// That makes bare `muster` an output-only check on the renderer, and it says so rather than
  /// presenting a terminal that ignores the keyboard.
  private func explainRendererCheck() {
    FileHandle.standardError.write(
      Data(
        """
        muster: no pane named, so this window only proves the renderer works.
        It runs $SHELL and paints what that prints, but every keystroke is dropped - \
        input needs a daemon-owned pane to encode for. To type into one, pass its id: \
        `muster w1:p1`. `herdr pane list` names the panes that exist.

        """.utf8))
  }

  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
