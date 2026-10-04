import AppKit
import os

/// One of this process's windows, as the core's events reach it.
///
/// A protocol rather than `MusterWindow` itself so that which window an event goes to can be
/// tested without a renderer: a real window needs a GPU and a libghostty runtime, and routing is
/// a question about names.
@MainActor
public protocol ShellWindow: AnyObject {
  /// What the core calls this window (`window-2`), which every event for it carries.
  var name: String { get }
  /// The platform window, which is how the window in front is found.
  var platformWindow: NSWindow { get }

  func apply(_ contents: WindowContents)
  func apply(_ roster: Roster)
  func apply(presentation: Presentation)
  func apply(pane: PaneKey, agent: PaneAgent)
  func apply(daemon: String, health: String, detail: String)
  func apply(appearance: Core.Appearance)
  func apply(bindings: [Core.Binding])
  func apply(problems: [Problem])
  func apply(groups: [SidebarModel.MessageGroup])
  func hold(_ held: HeldPaste)
  /// Whether this window lists the pane, which decides where a held paste is asked about.
  func lists(_ pane: PaneKey) -> Bool
  func raise()
  /// Closes as its close button does, so the last window's close is still a quit.
  func close()
}

/// Every window this process has open, by the name the core gave each (mip/0006-one-process.md).
///
/// Holds them, rather than only knowing of them: a window is closed by taking it out of here,
/// which is what lets go of its surfaces and their bridges.
@MainActor
public enum Windows {
  private static var open: [ShellWindow] = []

  /// Takes a window on once the core has named it. Events for it are delivered from here on.
  public static func register(_ window: ShellWindow) {
    open.removeAll { $0 === window }
    open.append(window)
    if Core.windowInFront.isEmpty { cameToTheFront(window) }
  }

  /// Lets a window go, after it has closed.
  public static func remove(_ window: ShellWindow) {
    open.removeAll { $0 === window }
    if Core.windowInFront == window.name {
      Core.windowInFront = inFront?.name ?? open.last?.name ?? ""
    }
  }

  public static func named(_ name: String) -> ShellWindow? {
    open.first { $0.name == name }
  }

  public static var all: [ShellWindow] { open }

  /// The window somebody is looking at: the key window, or the main one when a panel is key,
  /// taking a sheet as the window it belongs to.
  public static var inFront: ShellWindow? {
    for candidate in [NSApp.keyWindow, NSApp.mainWindow].compactMap({ $0 }) {
      var window = candidate
      while let parent = window.sheetParent { window = parent }
      if let found = open.first(where: { $0.platformWindow === window }) { return found }
    }
    return nil
  }

  /// Says a window came to the front, so a request sent from no window in particular - a menu
  /// item, a chord - is about it.
  public static func cameToTheFront(_ window: ShellWindow) {
    Core.windowInFront = window.name
  }

  /// Forgets every window, for a test that registered some.
  static func forgetAll() {
    open.removeAll()
    Core.windowInFront = ""
  }
}

extension Core {
  /// Which window a request that names none is about.
  ///
  /// Read from whichever thread sends, and a request is sent from more than the main thread, so a
  /// lock rather than the main actor's isolation.
  private struct Addressing {
    /// The window whose code is sending, while it is (`speaking(for:_:)`).
    var speaking: [String] = []
    /// The window that came to the front last.
    var front = ""
  }

  private static let addressing = OSAllocatedUnfairLock(initialState: Addressing())

  /// The window that came to the front last, and so the one a menu item or a chord is about.
  public internal(set) static var windowInFront: String {
    get { addressing.withLock { $0.front } }
    set { addressing.withLock { $0.front = newValue } }
  }

  /// Runs `body` with every request it sends naming `window`, unless it names one itself.
  ///
  /// What a window wraps around everything it sends on its own account: its list's clicks, its
  /// context menus, its frame. A window behind another still sends - a click in a list, a frame
  /// settling - and the window in front is not the one it is about.
  @discardableResult
  public static func speaking<T>(for window: String, _ body: () throws -> T) rethrows -> T {
    // Empty is nobody in particular, which leaves the window in front to answer.
    if window.isEmpty { return try body() }
    addressing.withLock { $0.speaking.append(window) }
    defer { addressing.withLock { _ = $0.speaking.popLast() } }
    return try body()
  }

  /// Names the window a request is for when it names none: the window speaking, else the one in
  /// front. The core decides for itself when neither is known.
  static func address(_ request: inout Muster_Request) {
    guard request.window.isEmpty else { return }
    // A log line is about the app, and names no window.
    if case .logRecord = request.payload { return }
    request.window = addressing.withLock { $0.speaking.last ?? $0.front }
  }
}

/// What the menu bar's items are aimed at: the window in front, found when an item is chosen or
/// checked rather than when the menu was built.
///
/// An explicit target rather than none, which would walk the responder chain from the surface
/// with the keyboard - and these actions are the window's, not a surface's (`AppMenu.entry`).
/// Forwarded rather than retargeted at each key change, so the menu is built once per set of
/// bindings whatever windows come and go.
public final class KeyWindowActions: NSObject, NSMenuItemValidation {
  @MainActor public static let shared = KeyWindowActions()

  /// The window an item acts on, or none when no window is open.
  @MainActor private static var window: MusterWindow? {
    (Windows.inFront ?? Windows.named(Core.windowInFront)) as? MusterWindow
  }

  public override func responds(to selector: Selector!) -> Bool {
    super.responds(to: selector) || MusterWindow.instancesRespond(to: selector)
  }

  public override func forwardingTarget(for selector: Selector!) -> Any? {
    guard MusterWindow.instancesRespond(to: selector) else {
      return super.forwardingTarget(for: selector)
    }
    return MainActor.assumeIsolated { Self.window }
  }

  /// An item is off while there is no window for it to act on, so it is never sent to nobody.
  public func validateMenuItem(_ item: NSMenuItem) -> Bool {
    MainActor.assumeIsolated {
      guard let window = Self.window else { return false }
      return window.validateMenuItem(item)
    }
  }
}
