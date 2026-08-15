import AppKit
import Testing

@testable import MusterMac

// Every way a person can rearrange panes, and the request each one becomes.
//
// The seam is recorded rather than crossed, so what these assert on is the message that would
// have reached the core - which is the same message the CLI and an agent will send. A gesture
// wired to the wrong one of these is invisible on screen until you try it: a split that
// resizes, a close that focuses.

private final class RecordingDispatcher: Dispatcher, @unchecked Sendable {
  private(set) var requests: [Muster_Request] = []

  func dispatch(_ request: [UInt8]) -> [UInt8] {
    if let decoded = try? Muster_Request(serializedBytes: request) {
      requests.append(decoded)
    }
    var response = Muster_Response()
    response.ok = Muster_Ok()
    return (try? response.serializedBytes()) ?? []
  }
}

@MainActor
private func recorder() -> RecordingDispatcher {
  let recorder = RecordingDispatcher()
  Core.dispatcher = recorder
  return recorder
}

@Suite("pane actions cross the seam")
struct PaneActionTests {
  @MainActor
  @Test("clicking a pane asks for the keyboard rather than taking it")
  func aClickIsARequest() {
    // Which pane the keyboard feeds is the core's answer, so a click asks and the view that
    // comes back moves the responder. A view that focused itself would disagree with the core
    // the moment the core refused - and then keystrokes would go somewhere else than the
    // ring says.
    let recorder = recorder()
    let started = RegionView(frame: NSRect(x: 0, y: 0, width: 400, height: 300)) { _, _, _ in }
    started.apply(
      WindowContents.Region(
        id: "r0", daemon: "devenv", tab: "w1:t1", keyboardPane: "w1:p1",
        tree: .pane(.init(paneID: "w1:p1", controlSocketPath: "/tmp/a.sock")), zoomed: false),
      focused: true)
    let before = recorder.requests.count

    started.chrome(for: "w1:p1")?.surface.mouseDown(with: click())

    let sent = recorder.requests.dropFirst(before)
    #expect(sent.map { $0.focusPane.paneID } == ["w1:p1"])
    // And it says which `w1:p1`. Both daemons hand out that id, so a click that named only
    // the pane would be answered by whichever region the core searched first - which is a
    // keyboard landing on the wrong machine, silently.
    #expect(sent.map { $0.focusPane.daemonID } == ["devenv"])
  }

  @MainActor
  @Test("a divider drag says which daemon's tab it is moving")
  func aDragNamesItsDaemon() {
    // Tabs collide across daemons exactly as panes do, and a ratio applied to the wrong tab
    // resizes a split the user is not looking at.
    let recorder = recorder()
    let region = RegionView(frame: NSRect(x: 0, y: 0, width: 400, height: 300)) { _, _, _ in }
    region.apply(
      WindowContents.Region(
        id: "r0", daemon: "devenv", tab: "w1:t1", keyboardPane: "w1:p1",
        tree: .split(
          axis: .columns, ratio: 0.5,
          first: .pane(.init(paneID: "w1:p1", controlSocketPath: "/tmp/a.sock")),
          second: .pane(.init(paneID: "w1:p2", controlSocketPath: "/tmp/b.sock"))),
        zoomed: false),
      focused: true)
    let before = recorder.requests.count

    region.layoutSubtreeIfNeeded()
    let divider = region.subviews.compactMap { $0 as? DividerView }.first
    divider?.onDrag?([false], 0.3)

    let sent = recorder.requests.dropFirst(before)
    #expect(sent.map { $0.setSplitRatio.daemonID } == ["devenv"])
    #expect(sent.map { $0.setSplitRatio.tabID } == ["w1:t1"])
  }

  @MainActor
  @Test("a split names the arrangement it wants, not a direction")
  func aSplitAsksForAnAxis() {
    // herdr says right and down, which describe the moment of splitting. Muster says columns
    // and rows, which describe what the user is looking at afterwards - and the translation
    // belongs in the adapter, not in a menu item.
    let recorder = recorder()

    Core.split(axis: SplitAxis.columns.rawValue)
    Core.split(axis: SplitAxis.rows.rawValue)

    #expect(recorder.requests.map { $0.splitPane.axis } == ["columns", "rows"])
    // Empty means the pane the keyboard feeds, which is what a keybinding means. A menu that
    // named a pane would be naming one it had to track.
    #expect(recorder.requests.allSatisfy { $0.splitPane.paneID.isEmpty })
    // Zero is the daemon's own default share, which is what a keybinding wants.
    #expect(recorder.requests.allSatisfy { $0.splitPane.ratio == 0 })
  }

  @MainActor
  @Test("dragging a divider names it by the turns down to it")
  func aDragCarriesThePath() {
    // A divider has no id - it is a position in a shape that changes under it - so the path
    // is the whole address. A wrong one silently resizes a different split.
    let recorder = recorder()

    Core.setSplitRatio(daemonID: "local", tab: "w1:t1", path: [true, false], ratio: 0.25)

    #expect(recorder.requests.count == 1)
    let set = recorder.requests[0].setSplitRatio
    #expect(set.tabID == "w1:t1")
    #expect(set.path == [true, false])
    #expect(abs(set.ratio - 0.25) < 0.001)
  }

  @MainActor
  @Test("stepping the keyboard asks for a direction, not a pane")
  func steppingIsResolvedByTheCore() {
    // The shell does not get to decide what is next: the order is the tab's tree, which is
    // daemon truth. A shell that picked would also have to agree with the CLI about what
    // "next" means, and nothing would make it.
    let recorder = recorder()

    Core.focus(step: "next")
    Core.focus(step: "previous")

    #expect(recorder.requests.map { $0.focusRelative.direction } == ["next", "previous"])
  }
}

@Suite("the menu is where a macOS keybinding lives")
struct AppMenuTests {
  @MainActor
  @Test("every pane action has a shortcut and something to send it to")
  func paneItemsAreWired() {
    // A menu item whose target does not implement its selector is a menu item that renders
    // grayed out and does nothing, and the compiler has nothing to say about it.
    let window = NSObject()
    for item in AppMenu.paneItems {
      #expect(!item.key.isEmpty)
      #expect(item.modifiers.contains(.command))
      #expect(MusterWindow.instancesRespond(to: item.action))
      _ = window
    }
  }

  @MainActor
  @Test("splitting is bound to Ghostty's own shortcuts")
  func shortcutsMatchTheTerminalWeEmbed() {
    // Somebody arriving from the terminal Muster embeds should not have to learn a second set
    // of keys for the same actions.
    let byTitle = Dictionary(uniqueKeysWithValues: AppMenu.paneItems.map { ($0.title, $0) })

    #expect(byTitle["Split Right"]?.key == "d")
    #expect(byTitle["Split Right"]?.modifiers == [.command])
    #expect(byTitle["Split Down"]?.modifiers == [.command, .shift])
    #expect(byTitle["Close Pane"]?.key == "w")
  }

  @MainActor
  @Test("the menu carries quit, paste and the panes")
  func theMenuBarIsComplete() {
    // Without a menu at all, ⌘V is inert no matter what any view implements and the app
    // cannot be quit normally.
    let menu = AppMenu.build(target: NSApp)

    let titles = menu.items.compactMap { $0.submenu?.items.map(\.title) }.flatMap { $0 }
    #expect(titles.contains("Quit muster"))
    #expect(titles.contains("Paste"))
    #expect(titles.contains("Split Right"))
  }
}

private func click() -> NSEvent {
  NSEvent.mouseEvent(
    with: .leftMouseDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
    context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
}
