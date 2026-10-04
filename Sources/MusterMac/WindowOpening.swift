import AppKit
import MusterRenderer

/// Opens this app's windows, and keeps hold of them while they are open (mip/0006-one-process.md).
///
/// Every window of an install is a window of one process, so ⌘N, Reopen Closed Window, going to
/// a closed window's tab and `muster window new` all end here: an arrangement chosen, the core
/// asked to open a window onto it, and a `MusterWindow` built for the name it answers with. In
/// that order, and all on the main thread before returning, so the window is registered before
/// any event the core published for it is delivered.
@MainActor
public final class WindowOpening {
  private let renderer: Renderer
  private let executable: String

  /// Every window open, with the arrangement it writes: no other window may be opened onto one of
  /// these while it is open, and a closed window's becomes free to reopen.
  private var opened: [(window: MusterWindow, arrangement: String?)] = []

  /// The arrangements the open windows write.
  public var arrangementsOpen: Set<String> { Set(opened.compactMap(\.arrangement)) }

  public init(renderer: Renderer, executable: String) {
    self.renderer = renderer
    self.executable = executable
  }

  /// Takes on a window opened some other way: the launch's first, which `Startup` described.
  public func adopt(_ window: MusterWindow, arrangement: String?) {
    opened.append((window, arrangement))
    window.onClosed = { [weak self] in self?.closed($0) }
  }

  /// What the core, the menu or a chord asked for.
  public func open(_ asked: Core.WindowAsked) {
    if asked.any {
      comeForward(show: asked.show)
      return
    }
    if !asked.name.isEmpty, let open = Windows.named(asked.name) {
      // Already open here: a second notification click, or a reopen racing the first.
      Core.info("window.reopen.already_open", ["window": asked.name])
      open.raise()
      return
    }
    let named = asked.name.isEmpty ? nil : asked.name
    guard
      let arrangement = Arrangements.open(
        fresh: asked.fresh, named: named, open: arrangementsOpen)
    else {
      Core.warn(
        "window.open.unremembered",
        [
          "window": asked.name,
          "impact": "no window opened, and the windows already open are unaffected",
          "check": "whether this Muster has a home to keep its windows in: HOME or MUSTER_HOME, "
            + "and MUSTER_STATE if it is set",
        ])
      NSSound.beep()
      return
    }
    // Asked for by name and not given that record: no closed window is called that, and a window
    // opened onto some other record would be a window nobody asked for.
    if let named,
      URL(fileURLWithPath: arrangement).deletingPathExtension().lastPathComponent != named
    {
      Core.info("window.reopen.unknown", ["window": named])
      NSSound.beep()
      return
    }
    open(arrangement: arrangement, show: asked.show, daemon: asked.daemon, tab: asked.tab)
  }

  /// Brings the app forward onto the window in front, or opens one when none is open: what a
  /// second launch asking for nothing in particular, and a click on the Dock icon, both mean.
  ///
  /// One that opens is the most recently closed window, as Reopen Closed Window is, so a Dock click
  /// after closing the last window but one brings that window back rather than an empty one. It
  /// opens onto `show`, a pane or tab a launch named: with a window already open the core has gone
  /// there, and with none it could not.
  public func comeForward(show: String = "") {
    guard let window = Windows.inFront ?? Windows.all.last else {
      Core.info("app.forward.opening", ["show": show])
      open(Core.WindowAsked(name: "", show: show, fresh: false))
      return
    }
    Core.info("app.forward", ["window": window.name])
    NSApp.activate(ignoringOtherApps: true)
    window.raise()
  }

  /// Opens a window onto one of the windows open when Muster last ended.
  public func reopen(arrangement: String) {
    open(arrangement: arrangement, show: "")
  }

  /// Opens a window onto an arrangement no window here has open.
  private func open(arrangement: String, show: String, daemon: String = "", tab: String = "") {
    guard let name = Core.open(arrangement: arrangement, show: show, daemon: daemon, tab: tab)
    else {
      NSSound.beep()
      return
    }
    if let open = Windows.named(name) {
      open.raise()
      return
    }
    let window = MusterWindow(renderer: renderer, executable: executable)
    adopt(window, arrangement: arrangement)
    window.opened(as: name)
    window.show()
    // Asked for from outside the app as often as from inside it - `muster window new` in a
    // terminal, a notification about a closed window's agent - and a window opened behind
    // whatever is in front is not one anybody can see they got.
    NSApp.activate(ignoringOtherApps: true)
  }

  /// Forgets a closed window, which frees its arrangement for a reopen; its tabs stay its own.
  private func closed(_ window: MusterWindow) {
    opened.removeAll { $0.window === window }
  }
}
