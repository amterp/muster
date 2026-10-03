import AppKit
import Testing

@testable import MusterMac

// Several windows in one process (mip/0006-one-process.md): which window a request names, and
// which window an event reaches. The windows here are stand-ins, since a real one needs a
// renderer; what is under test is the routing by name, which is the shell's alone.

@Suite("several windows in one process", .ownsTheSeam)
@MainActor
struct WindowsTests {
  @Test("a request names the window in front, or the window it was sent for")
  func aRequestNamesItsWindow() {
    Windows.forgetAll()
    defer { Windows.forgetAll() }
    let recorder = recorder()
    let first = StandIn("window-1")
    let second = StandIn("window-2")
    Windows.register(first)
    Windows.register(second)
    Windows.cameToTheFront(first)
    let mark = recorder.requests.count

    Core.toggleSidebar()
    Core.speaking(for: "window-2") { Core.toggleSidebar() }
    Core.info("windows.test", [:])

    let toggled = recorder.sent(since: mark) {
      if case .toggleSidebar = $0.payload { true } else { false }
    }
    #expect(toggled.map(\.window) == ["window-1", "window-2"])
    let logged = recorder.sent(since: mark) {
      if case .logRecord = $0.payload { true } else { false }
    }
    #expect(logged.allSatisfy { $0.window.isEmpty }, "a log line was sent as one window's")
  }

  /// The two senders that coalesce a drag go round `Core.send`, and still name their window.
  @Test("a frame reported from a window behind names that window")
  func aFrameNamesItsWindow() async {
    Windows.forgetAll()
    defer { Windows.forgetAll() }
    let recorder = recorder()
    Windows.register(StandIn("window-1"))
    Windows.cameToTheFront(StandIn("window-1"))
    let frames = WindowFrameSender(dispatcher: recorder)
    var answered = false
    frames.onAnswered = { answered = true }
    let mark = recorder.requests.count

    Core.speaking(for: "window-2") {
      frames.send(rect: NSRect(x: 0, y: 0, width: 10, height: 10), fullScreen: false)
    }
    await until("the frame to be answered") { answered }

    let sent = recorder.sent(since: mark) {
      if case .setWindowFrame = $0.payload { true } else { false }
    }
    #expect(sent.map(\.window) == ["window-2"])
  }

  @Test("a view reaches the window it names, and an untagged event reaches every window")
  func eventsFindTheirWindow() {
    Windows.forgetAll()
    defer { Windows.forgetAll() }
    let first = StandIn("window-1")
    let second = StandIn("window-2")
    Windows.register(first)
    Windows.register(second)

    var view = Muster_Event()
    view.viewChanged = Muster_ViewChanged()
    view.window = "window-2"
    Core.deliver(view)
    var problems = Muster_Event()
    problems.problemsChanged = Muster_ProblemsChanged()
    Core.deliver(problems)

    #expect(first.applied == ["problems"])
    #expect(second.applied == ["view", "problems"])
  }

  @Test("a view for a window this process does not have reaches no window")
  func aViewForNoWindowIsDropped() {
    Windows.forgetAll()
    defer { Windows.forgetAll() }
    let only = StandIn("window-1")
    Windows.register(only)

    var view = Muster_Event()
    view.viewChanged = Muster_ViewChanged()
    view.window = "window-9"
    Core.deliver(view)

    #expect(only.applied.isEmpty, "another window's view was drawn in this one")
  }

  /// The menu bar is built once and aimed at whichever window is in front when an item is used,
  /// and an item with no window to act on is off rather than sent to nobody.
  @Test("the menu bar's target answers for the window in front")
  func theMenuBarsTarget() {
    Windows.forgetAll()
    defer { Windows.forgetAll() }
    let target = KeyWindowActions.shared
    #expect(target.responds(to: #selector(MusterWindow.newTab(_:))))
    #expect(!target.responds(to: NSSelectorFromString("noSuchAction:")))

    let item = NSMenuItem(
      title: "New Tab", action: #selector(MusterWindow.newTab(_:)), keyEquivalent: "")
    #expect(!target.validateMenuItem(item), "an item was on with no window to act on")
  }
}

/// A window that records what reached it.
@MainActor
private final class StandIn: ShellWindow {
  let name: String
  let platformWindow = NSWindow(
    contentRect: NSRect(x: 0, y: 0, width: 10, height: 10), styleMask: [.titled],
    backing: .buffered, defer: true)
  var applied: [String] = []

  init(_ name: String) { self.name = name }

  func apply(_ contents: WindowContents) { applied.append("view") }
  func apply(_ roster: Roster) { applied.append("roster") }
  func apply(presentation: Presentation) { applied.append("presentation") }
  func apply(pane: PaneKey, agent: PaneAgent) { applied.append("agent") }
  func apply(daemon: String, health: String, detail: String) { applied.append("health") }
  func apply(appearance: Core.Appearance) { applied.append("appearance") }
  func apply(bindings: [Core.Binding]) { applied.append("bindings") }
  func apply(problems: [Problem]) { applied.append("problems") }
  func hold(_ held: HeldPaste) { applied.append("paste") }
  func lists(_ pane: PaneKey) -> Bool { false }
  func raise() { applied.append("raise") }
}
