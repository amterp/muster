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
  @Test("a new tab names neither a pane nor a directory, and means both")
  func aNewTabMeansHere() {
    // Everything empty is the message a keybinding sends: the pane the keyboard is on, and
    // the directory that pane is in. Both are resolved in the core, so a menu item, the CLI
    // and an agent get the same answer rather than three.
    let recorder = recorder()

    Core.createTab()

    #expect(recorder.requests.count == 1)
    guard case .createTab(let create) = recorder.requests.first?.payload else {
      Issue.record("a new tab sent \(String(describing: recorder.requests.first?.payload))")
      return
    }
    #expect(create.paneID.isEmpty)
    #expect(create.cwd.isEmpty)
    #expect(create.daemonID.isEmpty)
  }

  @MainActor
  @Test("clicking a pane asks for the keyboard rather than taking it")
  func aClickIsARequest() {
    // Which pane the keyboard feeds is the core's answer, so a click asks and the view that
    // comes back moves the responder. A view that focused itself would disagree with the core
    // the moment the core refused - and then keystrokes would go somewhere else than the
    // ring says.
    let recorder = recorder()
    let started = RegionView(frame: NSRect(x: 0, y: 0, width: 400, height: 300)) { _, _, _, _, _ in
    }
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
    let region = RegionView(frame: NSRect(x: 0, y: 0, width: 400, height: 300)) { _, _, _, _, _ in }
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
    divider?.onDrag?(0.3)

    let sent = recorder.requests.dropFirst(before)
    #expect(sent.map { $0.setSplitRatio.daemonID } == ["devenv"])
    #expect(sent.map { $0.setSplitRatio.tabID } == ["w1:t1"])
  }

  @MainActor
  @Test("a region drag names the region on its left, not a position")
  func aRegionDragNamesARegion() {
    // The one drag no daemon is told about: how a window divides between a laptop and a
    // devenv is Muster's own arrangement. Named by the region rather than by an index,
    // because a drag is a stream of requests and a region can close underneath one - an
    // index would then move some other line, silently.
    let recorder = recorder()
    let strip = RegionStrip(frame: NSRect(x: 0, y: 0, width: 1000, height: 600))
    strip.arrange([
      (id: "r0", weight: 1, view: NSView()),
      (id: "r1", weight: 1, view: NSView()),
    ])
    let before = recorder.requests.count

    strip.layoutSubtreeIfNeeded()
    let divider = strip.subviews.compactMap { $0 as? DividerView }.first
    #expect(divider != nil, "two regions have one line between them")
    divider?.onDrag?(0.25)

    let sent = recorder.requests.dropFirst(before)
    #expect(sent.map { $0.setRegionBoundary.regionID } == ["r0"])
    #expect(sent.map { $0.setRegionBoundary.ratio } == [0.25])
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

  @MainActor
  @Test("every direction the core knows is a word the shell can send")
  func everyDirectionCrossesTheSeam() {
    // The core refuses a direction it does not know, by name, and a refused focus is a dead
    // key - the failure this project has spent the most time on. Asserted as the words rather
    // than a count so that a spelling drifting on one side shows up here.
    let recorder = recorder()

    for direction in ["next", "previous", "left", "right", "up", "down"] {
      Core.focus(step: direction)
    }

    #expect(
      recorder.requests.map { $0.focusRelative.direction }
        == ["next", "previous", "left", "right", "up", "down"])
  }
}

@Suite("the menu is where a macOS keybinding lives")
struct AppMenuTests {
  /// What the core publishes, as it publishes it.
  ///
  /// A fixture rather than the core's own answer, because these test the translation and not
  /// the policy: which chord each action ships on is the core's, pinned in
  /// `corpus/conformance/bindings.json`. What is checked here is that a published binding
  /// becomes a menu item AppKit will actually dispatch.
  ///
  /// Drift between the two lists - an action the core has and this shell does not - is caught
  /// where it matters rather than here: building the menu logs `menu.action.unknown`, and a
  /// contract run fails on any warning it did not expect.
  static let published: [Core.Binding] = [
    Core.Binding(action: "new_tab", key: "KeyT", modifiers: ["super"]),
    Core.Binding(action: "split_right", key: "KeyD", modifiers: ["super"]),
    Core.Binding(action: "split_down", key: "KeyD", modifiers: ["shift", "super"]),
    Core.Binding(action: "close_pane", key: "KeyW", modifiers: ["super"]),
    Core.Binding(action: "next_pane", key: "BracketRight", modifiers: ["super"]),
    Core.Binding(action: "previous_pane", key: "BracketLeft", modifiers: ["super"]),
    Core.Binding(action: "focus_left", key: "ArrowLeft", modifiers: ["alt", "super"]),
    Core.Binding(action: "focus_right", key: "ArrowRight", modifiers: ["alt", "super"]),
    Core.Binding(action: "focus_up", key: "ArrowUp", modifiers: ["alt", "super"]),
    Core.Binding(action: "focus_down", key: "ArrowDown", modifiers: ["alt", "super"]),
    Core.Binding(
      action: "resize_left", key: "ArrowLeft", modifiers: ["shift", "control", "super"]),
    Core.Binding(
      action: "resize_right", key: "ArrowRight", modifiers: ["shift", "control", "super"]),
    Core.Binding(action: "resize_up", key: "ArrowUp", modifiers: ["shift", "control", "super"]),
    Core.Binding(
      action: "resize_down", key: "ArrowDown", modifiers: ["shift", "control", "super"]),
    Core.Binding(action: "zoom", key: "Enter", modifiers: ["shift", "super"]),
  ]

  @MainActor
  @Test("every published action becomes an item something can carry out")
  func paneItemsAreWired() {
    // A menu item whose target does not implement its selector renders grayed out and does
    // nothing, and the compiler has nothing to say about it.
    let items = AppMenu.paneItems(AppMenuTests.published)
    #expect(items.count == AppMenuTests.published.count)
    for item in items {
      #expect(!item.key.isEmpty)
      #expect(item.modifiers.contains(.command))
      #expect(MusterWindow.instancesRespond(to: item.action))
    }
    // And no two items are the same action or the same chord. Four directional items written
    // one after another is exactly where a copy-paste points two titles at one selector, and
    // the symptom is a key that quietly moves the wrong way rather than one that does nothing.
    let actions = Set(items.map { NSStringFromSelector($0.action) })
    #expect(actions.count == items.count)
    let chords = Set(items.map { "\($0.modifiers.rawValue):\($0.key)" })
    #expect(chords.count == items.count)
  }

  @MainActor
  @Test("a rebound chord moves the menu item, which is what rebinding means on macOS")
  func rebindingMovesTheItem() {
    // The whole point of the menu being built from the core: a config file that moves an
    // action moves its key equivalent, because on macOS the key equivalent is the binding.
    let items = AppMenu.paneItems([
      Core.Binding(action: "split_right", key: "Backslash", modifiers: ["control"])
    ])

    #expect(items.count == 1)
    #expect(items[0].title == "Split Right")
    #expect(items[0].key == "\\")
    #expect(items[0].modifiers == [.control])
  }

  @MainActor
  @Test("an unbound action keeps its item and loses its shortcut")
  func unbindingLeavesTheAction() {
    // Somebody who unbound ⌘W wanted the shortcut back, not the action gone - and a menu is
    // also where you look when you have forgotten what something is called.
    let items = AppMenu.paneItems([Core.Binding(action: "close_pane", key: "", modifiers: [])])

    #expect(items.count == 1)
    #expect(items[0].title == "Close Pane")
    #expect(items[0].key.isEmpty)
  }

  @MainActor
  @Test("an action this shell has never heard of is left out rather than guessed at")
  func anUnknownActionIsSkipped() {
    // A core one version ahead. A menu missing a line is recoverable; a crash at launch is
    // not, and neither is an item pointed at a selector nobody implements.
    let items = AppMenu.paneItems([
      Core.Binding(action: "teleport", key: "KeyT", modifiers: ["super"]),
      Core.Binding(action: "zoom", key: "Enter", modifiers: ["shift", "super"]),
    ])

    #expect(items.map(\.title) == ["Zoom Pane"])
  }

  @MainActor
  @Test("the keys AppKit needs are the ones it gets")
  func keyEquivalentsAreWhatAppKitWants() {
    // AppKit takes a character rather than a key code, and it spells shift as a capital in
    // this field - so a capital here plus .shift in the mask is an item needing shift twice.
    #expect(menuKeyEquivalent(forKeyNamed: "KeyD") == "d")
    #expect(menuKeyEquivalent(forKeyNamed: "Digit1") == "1")
    #expect(menuKeyEquivalent(forKeyNamed: "ArrowLeft") == "\u{F702}")
    #expect(menuKeyEquivalent(forKeyNamed: "Enter") == "\r")
    #expect(menuKeyEquivalent(forKeyNamed: "BracketRight") == "]")
    // No character to put on a menu. An empty key equivalent is an item with no shortcut,
    // which is the honest rendering of a chord AppKit cannot express.
    #expect(menuKeyEquivalent(forKeyNamed: "AudioVolumeUp") == nil)
  }

  @MainActor
  @Test("the menu carries quit, copy, paste and the panes")
  func theMenuBarIsComplete() {
    // Without a menu at all, ⌘V is inert no matter what any view implements and the app
    // cannot be quit normally.
    let menu = AppMenu.build(target: NSApp, bindings: AppMenuTests.published)

    let titles = menu.items.compactMap { $0.submenu?.items.map(\.title) }.flatMap { $0 }
    #expect(titles.contains("Quit muster"))
    #expect(titles.contains("Paste"))
    #expect(titles.contains("Split Right"))
    // Copy went missing for as long as a pane's selection was believed to live in the
    // daemon. Reading a terminal's output back out is an hourly need, and without this the
    // answer was retyping it.
    #expect(titles.contains("Copy"))
  }

  @MainActor
  @Test("copy and paste go through the responder chain, not to a target")
  func editItemsWalkTheChain() {
    // A target here would be whichever object built the menu, and the selection belongs to
    // the focused surface. Nil is what makes AppKit walk down to it.
    let menu = AppMenu.build(target: NSApp, bindings: AppMenuTests.published)
    let edit = menu.items.compactMap(\.submenu).first { $0.title == "Edit" }

    let items = edit?.items.filter { ["Copy", "Paste"].contains($0.title) } ?? []
    #expect(items.count == 2)
    #expect(items.allSatisfy { $0.target == nil })
  }
}

private func click() -> NSEvent {
  NSEvent.mouseEvent(
    with: .leftMouseDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
    context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
}
