import AppKit
import MusterRenderer

/// Opens this app's windows, and keeps hold of them while they are open (mip/0006-one-process.md).
///
/// Every window of an install is a window of one process, so ⌘N, Reopen Closed Window, going to
/// a closed window's tab and `muster window new` all end here: an arrangement claimed, the core
/// asked to open a window onto it, and a `MusterWindow` built for the name it answers with. In
/// that order, and all on the main thread before returning, so the window is registered before
/// any event the core published for it is delivered.
@MainActor
public final class WindowOpening {
  private let renderer: Renderer
  private let executable: String

  /// Every window this opened, with the arrangement it claimed - released when it closes, so a
  /// later launch, or a reopen, can take it.
  private var opened: [(window: MusterWindow, claimed: String?)] = []

  public init(renderer: Renderer, executable: String) {
    self.renderer = renderer
    self.executable = executable
  }

  /// Takes on a window opened some other way: the launch's first, which `Startup` described.
  public func adopt(_ window: MusterWindow, claimed: String?) {
    opened.append((window, claimed))
    window.onClosed = { [weak self] in self?.closed($0) }
  }

  /// What the core, the menu or a chord asked for.
  public func open(_ asked: Core.WindowAsked) {
    if asked.any {
      comeForward()
      return
    }
    if !asked.name.isEmpty, let open = Windows.named(asked.name) {
      // Already open here: a second notification click, or a reopen racing the first.
      Core.info("window.reopen.already_open", ["window": asked.name])
      open.raise()
      return
    }
    let named = asked.name.isEmpty ? nil : asked.name
    guard let arrangement = Arrangements.open(fresh: asked.fresh, named: named) else {
      Core.warn(
        "window.open.unclaimed",
        [
          "window": asked.name,
          "impact": "no window opened, and the windows already open are unaffected",
          "check": "whether Muster's state directory is writable, and whether the window named "
            + "is open in another Muster",
        ])
      NSSound.beep()
      return
    }
    // Asked for by name and not given that record: it is held by a window open elsewhere, and a
    // window opened onto some other record would be a window nobody asked for.
    if let named,
      URL(fileURLWithPath: arrangement).deletingPathExtension().lastPathComponent != named
    {
      Arrangements.release(arrangement)
      Core.info("window.reopen.held_elsewhere", ["window": named])
      return
    }
    open(arrangement: arrangement, show: asked.show)
  }

  /// Brings the app forward onto the window in front, or opens one when none is open: what a
  /// second launch asking for nothing in particular, and a click on the Dock icon, both mean.
  ///
  /// One that opens is the most recently closed window, as Reopen Closed Window is, so a Dock click
  /// after closing the last window but one brings that window back rather than an empty one.
  public func comeForward() {
    guard let window = Windows.inFront ?? Windows.all.last else {
      Core.info("app.forward.opening", [:])
      open(Core.WindowAsked(name: "", show: "", fresh: false))
      return
    }
    Core.info("app.forward", ["window": window.name])
    NSApp.activate(ignoringOtherApps: true)
    window.raise()
  }

  /// Opens a window onto an arrangement this launch has already claimed: one of the windows open
  /// when Muster last ended.
  public func reopen(claimed arrangement: String) {
    open(arrangement: arrangement, show: "")
  }

  /// Opens a window onto a claimed arrangement.
  private func open(arrangement: String, show: String) {
    guard let name = Core.open(arrangement: arrangement, show: show) else {
      Arrangements.release(arrangement)
      NSSound.beep()
      return
    }
    if let open = Windows.named(name) {
      open.raise()
      return
    }
    let window = MusterWindow(renderer: renderer, executable: executable)
    adopt(window, claimed: arrangement)
    window.opened(as: name)
    window.show()
    // Asked for from outside the app as often as from inside it - `muster window new` in a
    // terminal, a notification about a closed window's agent - and a window opened behind
    // whatever is in front is not one anybody can see they got.
    NSApp.activate(ignoringOtherApps: true)
  }

  /// Gives up a closed window's claim on its arrangement, which keeps its tabs for a reopen.
  private func closed(_ window: MusterWindow) {
    guard let at = opened.firstIndex(where: { $0.window === window }) else { return }
    if let claimed = opened[at].claimed { Arrangements.release(claimed) }
    opened.remove(at: at)
  }

  /// Gives up every claim, on the way out. The windows stay open in the record, which is what the
  /// next launch reopens; the claims only stop two live processes taking one arrangement.
  public func releaseEveryClaim() {
    for (_, claimed) in opened {
      if let claimed { Arrangements.release(claimed) }
    }
  }
}
