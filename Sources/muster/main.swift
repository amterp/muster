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
  /// Every window, from the first on.
  private var windows: WindowOpening?
  /// What asking window processes from before to quit came to, before the app ran.
  var retiring = Retiring.Outcome()
  private var renderer: Renderer?
  /// Held for the life of the app; dropping it stops the watch.
  private var watcher: ConfigWatcher?
  /// Hears another window take a tab, give one away, open or close.
  private var holdingWatcher: ConfigWatcher?

  func applicationDidFinishLaunching(_ notification: Notification) {
    // Before anything is started, so that from here a SIGTERM is a quit rather than a death.
    TerminationSignal.quitsTheApp()

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
    // its own daemon rather than asking anybody to install one, and where a build put it is
    // an OS question while starting it is the core's.
    // And the state file, which is the same division once more: where a window's arrangement
    // is remembered is an OS question, and what is worth remembering is the core's.
    // The endpoint is the division once more with a resource rather than a file: what a request
    // means is the core's, and where a caller on this machine should look for this process is
    // an OS question - which is why the pid in the name is decided here.
    let config = configPath()
    let daemon = daemonLocation(executable: CommandLine.arguments[0])
    // Sockets from Musters that were killed refuse every connection, and finding the endpoint
    // means trying the ones that are there - so left alone they make the CLI slower to answer
    // and harder to trust with every crash.
    sweepDeadCommandSockets()
    // Refreshed at every launch rather than installed once: the CLI this points at moves with the
    // app, and a stale `muster` on somebody's PATH would talk to a window it no longer matches.
    // Nil means this build staged none, and the core is then told nothing rather than a directory
    // holding a link that cannot run.
    let commands = refreshMusterCommand(
      executable: CommandLine.arguments[0], commands: commandsPath())
    // A window somebody asked for remembers its tabs under a record of its own, which the shell
    // picks and claims for as long as this process runs - an OS question. Which tabs it holds is
    // the core's, and a window starts holding nothing until it asks for a tab of its own.
    //
    // A plain launch comes back to every window that was open when Muster last ended - a quit, a
    // crash, a reboot - each onto its own arrangement (mip/0006-one-process.md, section 4). The
    // first becomes the window `Startup` describes, and the rest open beside it once it has. With
    // none to come back to, which is a first launch or the first after upgrading from a Muster
    // that closed every window on the way out, the launch takes one arrangement as it always did.
    let launched = Array(CommandLine.arguments.dropFirst())
    let fresh = launchIsFresh(arguments: launched)
    let holders = tabHoldersPath()
    let reopensEvery = launchReopensEveryWindow(
      arguments: launched, environment: ProcessInfo.processInfo.environment)
    let restored = reopensEvery ? Core.reopening(tabHoldersPath: holders) : []
    // The windows of processes from before that quit when asked come back here too, whatever this
    // launch was asked to be: they were open, and a process from before marked them closed on the
    // way out as every quit used to (mip/0006-one-process.md, section 6).
    let retired = Retiring.arrangements(retiring, in: InstallState.directory)
      .filter { !restored.contains($0) }
    let reopening = (restored + retired).filter { Arrangements.take($0) }
    // A launch told what to be opens that first, and the windows it comes back to beside it.
    let first = reopensEvery ? reopening.first : nil
    let arrangement =
      first ?? Arrangements.open(fresh: fresh, named: launchWindow(arguments: launched))
    let reopened = first == nil ? reopening : Array(reopening.dropFirst())
    Core.start(
      logPath: logPath, configPath: config, daemon: daemon, statePath: arrangement,
      commandSocketPath: commandSocketPath(), commandsPath: commands,
      daemonRecordsPath: daemonRecordsPath(), tabHoldersPath: holders,
      show: launchShow(arguments: launched))
    watchTabHolders(holders)
    if let refused = Arrangements.linkRefused {
      Core.warn(
        "arrangement.claim.unlinked",
        [
          "detail": refused,
          "impact":
            "this filesystem refused the link that makes a window's claim on its arrangement "
            + "exclusive, so claims were made another way that two launches in the same moment "
            + "can both win",
          "check": "whether Muster's home is on a network or FAT volume; set MUSTER_HOME to a "
            + "local APFS directory",
        ])
    }
    // Every window's claim is given up on the way out, so a relaunch in the same second finds
    // the records rather than the claims. Not relied on: a process that is killed never gets
    // here, and a claim carries a pid for exactly that.
    NotificationCenter.default.addObserver(
      forName: NSApplication.willTerminateNotification, object: nil, queue: .main
    ) { [weak self] _ in
      MainActor.assumeIsolated { self?.windows?.releaseEveryClaim() }
    }
    Core.info(
      "app.launch",
      [
        "args": CommandLine.arguments.dropFirst().joined(separator: " "),
        "config": config ?? "(none)",
        "input_recorded": String(Core.includesInput),
      ])
    if !retiring.quit.isEmpty {
      Core.info("app.retired", ["windows": retiring.quit.joined(separator: " ")])
    }
    if !retiring.stayed.isEmpty {
      Core.warn(
        "app.retire.refused",
        [
          "pids": retiring.stayed.map(String.init).joined(separator: " "),
          "impact": "a Muster window from before every window shared one app is still running "
            + "beside this one, and the two can show the same tabs and fight over their "
            + "terminals",
          "check": "quit that window yourself (it is its own Dock icon), or `kill` the pid",
        ])
    }
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

      let windows = WindowOpening(renderer: renderer, executable: CommandLine.arguments[0])
      self.windows = windows
      let muster = MusterWindow(renderer: renderer, executable: CommandLine.arguments[0])
      windows.adopt(muster, claimed: arrangement)
      AppMenu.install(target: KeyWindowActions.shared, bindings: Core.bindings())
      Core.openWindowAsked = { [weak windows] asked in windows?.open(asked) }
      muster.show()

      // After the window is up, because nothing about it is needed to draw one and asking
      // CoreText for a font it has never been asked about costs a few milliseconds. The core
      // decides what a missing family means; only this side can find out whether it is here.
      Core.reportFontFamily(appearance.pane.fontFamily)

      // Everything about what this window shows is behind these calls: the core reaches the
      // daemons, starting its own if none answers, opens a socket per pane, and publishes the
      // whole view back - which is what builds the surfaces.
      let attached: Bool
      switch launchRequest(arguments: Array(CommandLine.arguments.dropFirst())) {
      case .open:
        let opened = Core.open()
        attached = muster.opened(as: opened)
        if !attached {
          muster.report(problem: "no session could be opened (see stderr)")
        }
        // In the order they were focused, so the window somebody last looked at opens last and
        // is in front.
        for arrangement in reopened {
          windows.reopen(claimed: arrangement)
        }
      case .pane(let paneID):
        attached = muster.opened(as: Core.attach(paneID: paneID))
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

      // After the window, because the permission prompt should land over a Muster somebody
      // can see rather than over whatever they were doing. Nothing else waits on it: an
      // agent that needs somebody before the answer comes back is still on its row.
      PaneNotifier.shared.start()

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
  /// Watches the record of which window holds each tab, so a tab another window takes leaves this
  /// one and a tab given to this one arrives.
  ///
  /// Its directory is made first, because a watch is on the directory and the first window ever
  /// to open finds none. The core writes into it moments later.
  private func watchTabHolders(_ path: String?) {
    guard let path else { return }
    let directory = URL(fileURLWithPath: path).deletingLastPathComponent()
    try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let watcher = ConfigWatcher(path: path) { Core.readTabHolders() }
    holdingWatcher = watcher
    if !watcher.start() {
      Core.warn(
        "holding.watch.failed",
        [
          "path": path,
          "impact": "this window will not hear another window take one of its tabs, so it can "
            + "keep drawing a tab that has moved and fight the other window for its terminals",
          "check": "whether the directory holding it exists and is readable",
        ])
    }
  }

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

  /// A click on the Dock icon while the app runs: AppKit restores a minimised window itself, and
  /// with none open at all the app opens one (mip/0006-one-process.md, section 5).
  func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
    guard Windows.all.isEmpty else { return true }
    windows?.comeForward()
    return false
  }

  /// No: the last window's close is turned into a quit before it happens
  /// (`MusterWindow.windowShouldClose`), and a window closing for any other reason - a minimised
  /// one among them - is not the app ending.
  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
}

// Before anything asks where Muster's files are, because everything reads it through the
// environment and this is the one moment it can still be answered. A window opened by an older
// `muster window new` is started through LaunchServices, which hands over no environment, so the
// home it was told about arrives on the command line instead - and is put back into the
// environment here rather than threaded through the places that ask.
//
// The environment wins where this process already has one, which is somebody who set it up
// deliberately: a test, or a Muster launched by hand.
let launched = Array(CommandLine.arguments.dropFirst())
if ProcessInfo.processInfo.environment["MUSTER_HOME"] == nil,
  let home = launchHome(arguments: launched)
{
  setenv("MUSTER_HOME", home, 1)
}

// Before the app runs, so a launch that is not the app never shows a Dock icon: one process is
// the app of its install, and a second launch hands what it was asked to do to that one and exits
// (mip/0006-one-process.md, section 5).
var retiredAtLaunch = Retiring.Outcome()
if let asking = handOver(arguments: launched) {
  switch Core.claimApp(
    home: musterHome()?.path, commandSocketPath: commandSocketPath(), asking: asking)
  {
  case .claimed(let state):
    InstallState.directory = state.map { URL(fileURLWithPath: $0, isDirectory: true) }
    // Once this is the app, and before anything reads its state: a process from before writes
    // the old shared directory until it has quit, and adopting that directory first would take it
    // from under a window still writing it.
    retiredAtLaunch = Retiring.olderWindowProcesses(home: musterHome())
    Core.adoptOldState(home: musterHome()?.path)
  case .handedOver:
    FileHandle.standardError.write(Data("muster: handed to the Muster already running\n".utf8))
    exit(0)
  case .refused(let reason):
    FileHandle.standardError.write(Data("muster: \(reason)\n".utf8))
    exit(1)
  }
}

let app = NSApplication.shared
let delegate = AppDelegate()
delegate.retiring = retiredAtLaunch
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
