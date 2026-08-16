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
  /// Held for the life of the app; dropping it stops the watch.
  private var watcher: ConfigWatcher?

  func applicationDidFinishLaunching(_ notification: Notification) {
    // Before the core, because the core attaches daemons as it starts and every one of those
    // opens sockets. Reported a few lines further down, once there is somewhere to report to.
    let descriptors = DescriptorLimit.raise()

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
    // The daemon binary goes over for the same reason and at the same moment: Muster runs
    // its own herdr rather than asking anybody to install one, and where a build put it is
    // an OS question while starting it is the core's.
    // And the state file, which is the same division once more: where a window's arrangement
    // is remembered is an OS question, and what is worth remembering is the core's.
    let config = configPath()
    let daemon = herdrPath(executable: CommandLine.arguments[0])
    Core.start(
      logPath: logPath, configPath: config, herdrPath: daemon, statePath: statePath())
    Core.info(
      "app.launch",
      [
        "args": CommandLine.arguments.dropFirst().joined(separator: " "),
        "config": config ?? "(none)",
        "input_recorded": String(Core.includesInput),
      ])
    if let stranded = strandedConfigPath() {
      Core.warn(
        "config.moved",
        [
          "found": stranded,
          "expected": "$MUSTER_HOME/config.toml, or ~/.muster/config.toml",
          "impact":
            "none of it was read, so this window is attached to whatever Muster could find "
            + "for itself and every keymap, appearance and typing setting is the default",
          "fix": "move the file to ~/.muster/config.toml",
        ])
    }
    DescriptorLimit.report(descriptors)

    do {
      // The core decides what the window should look like, because that is what the config
      // file said; the shell decides where the renderer's derived copy of it goes, because
      // that is an OS question - the same division every other path here draws.
      // One read, two halves: the renderer paints inside a pane and Muster paints the line
      // between two of them. After this the core sends the same answer as an event whenever
      // the file is read again.
      let appearance = Core.appearance()
      adoptChrome(appearance)
      let renderer = try Renderer(
        appearance: appearance.pane, configPath: rendererConfigPath())
      for complaint in renderer.diagnostics {
        Core.warn(
          "renderer.config.rejected",
          [
            "complaint": complaint,
            "impact": "that one setting is the renderer's own default; everything else applied",
            "fix":
              "a bug in Muster's translation rather than in the config file, which the core "
              + "already parsed - report it with this line",
          ])
      }
      Renderer.current = renderer
      self.renderer = renderer

      let muster = MusterWindow(renderer: renderer, executable: CommandLine.arguments[0])
      Core.window = muster
      self.muster = muster
      NSApp.mainMenu = AppMenu.build(target: muster, bindings: Core.bindings())
      muster.show()

      // Everything about what this window shows is behind these calls: the core reaches the
      // daemons, starting its own if none answers, opens a socket per pane, and publishes the
      // whole view back - which is what builds the surfaces.
      let attached: Bool
      switch launchRequest(arguments: Array(CommandLine.arguments.dropFirst())) {
      case .open:
        attached = Core.open()
        if !attached {
          muster.report(problem: "no session could be opened (see stderr)")
        }
      case .pane(let paneID):
        attached = Core.attach(paneID: paneID)
        if !attached {
          muster.report(problem: "\(paneID) could not be attached (see stderr)")
        }
      case .rendererCheck:
        explainRendererCheck()
        muster.showRendererCheck()
        attached = false
      case .unknown(let flag):
        explainUnknownFlag(flag)
        NSApp.terminate(nil)
        return
      }
      // After the window is up, so a save landing during launch cannot ask for a reload
      // before there is anything to repaint. Nothing to watch when no config file was found:
      // the reload action still works and finds nothing, which is the same answer.
      if let config {
        let watcher = ConfigWatcher(path: config) { Core.reloadConfig() }
        self.watcher = watcher
        if !watcher.start() {
          Core.warn(
            "config.watch.failed",
            [
              "path": config,
              "impact": "editing the config file will not take effect on its own; the Reload "
                + "Configuration menu item and its chord still work",
              "check": "whether the directory holding it is readable",
            ])
        }
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

  /// `--renderer-check` has no daemon behind it, and Muster's input path only knows how to
  /// talk to one: it encodes a keystroke and hands the bytes to a pane's control stream. A
  /// local shell has no control stream, so there is nowhere to put them.
  ///
  /// That makes this an output-only check on the renderer, and it says so rather than
  /// presenting a terminal that ignores the keyboard.
  private func explainRendererCheck() {
    FileHandle.standardError.write(
      Data(
        """
        muster: --renderer-check, so this window only proves the renderer works.
        It runs $SHELL and paints what that prints, but every keystroke is dropped - \
        input needs a daemon-owned pane to encode for. Run `muster` with no arguments \
        for an ordinary window.

        """.utf8))
  }

  /// A flag nobody reads is usually a misspelling of one somebody meant, so it is refused
  /// rather than ignored - the same rule the config file already applies to its own keys.
  private func explainUnknownFlag(_ flag: String) {
    FileHandle.standardError.write(
      Data(
        """
        muster: \(flag) is not something Muster reads, so nothing was opened.
        Run `muster` with no arguments for an ordinary window, `muster w1:p1` to start \
        the keyboard on a named pane, or `muster --renderer-check` for a window with no \
        daemon behind it.

        """.utf8))
  }

  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
