import AppKit
import Testing

@testable import MusterMac

// The right-click menus on a pane, a tab and an agent row, and the request each item becomes.
//
// Recorded at the seam, like every other gesture: what these assert is the message the core
// would get, which is the one a chord and the CLI send. The thing worth pinning hardest is that
// every item names the pane or tab that was right-clicked. A menu item left to "the one with the
// keyboard" looks right in every demo, because the pane you right-click is usually the one you
// were typing in, and splits the wrong pane the first time it is not.

@MainActor private let laptopPane = PaneKey(daemon: "laptop", pane: "p1w3r07bsd")

@MainActor private let bindings = [
  Core.Binding(action: "split_right", key: "KeyD", modifiers: ["super"]),
  Core.Binding(action: "close_pane", key: "KeyW", modifiers: ["super"]),
]

@Suite("right-click menus", .ownsTheSeam)
@MainActor
struct ContextMenuTests {
  @Test("a pane's menu offers Ghostty's items and Muster's own, in that order")
  func aPanesMenu() {
    _ = recorder()
    let menu = ContextMenus.pane(
      laptopPane, surface: nil, machines: ["laptop"], bindings: bindings, rename: { _ in })

    #expect(
      titles(menu) == [
        "Copy", "Paste", "-", "Split Right", "Split Down", "Split Left", "Split Up", "-",
        "Zoom Pane", "Move Pane to New Tab", "Rename Pane…", "-", "Copy Pane ID", "-",
        "Close Pane",
      ])
  }

  /// One machine has nothing to choose between, and most windows never attach a second.
  @Test("the machine submenu appears once a second machine is attached, listing every one")
  func theMachineSubmenu() {
    _ = recorder()
    let one = ContextMenus.pane(
      laptopPane, surface: nil, machines: ["laptop"], bindings: [], rename: { _ in })
    let two = ContextMenus.pane(
      laptopPane, surface: nil, machines: ["laptop", "devenv"], bindings: [], rename: { _ in })

    #expect(!titles(one).contains("Split on Machine"))
    #expect(titles(item("Split on Machine", in: two)?.submenu) == ["laptop", "devenv"])
  }

  @Test("an item shows the chord the core says its action is bound to")
  func chordsComeFromTheBindings() {
    _ = recorder()
    let rebound = [Core.Binding(action: "split_right", key: "KeyX", modifiers: ["super", "shift"])]

    let standard = item(
      "Split Right",
      in: ContextMenus.pane(
        laptopPane, surface: nil, machines: [], bindings: bindings, rename: { _ in }))
    let moved = item(
      "Split Right",
      in: ContextMenus.pane(
        laptopPane, surface: nil, machines: [], bindings: rebound, rename: { _ in }))

    #expect(standard?.keyEquivalent == "d")
    #expect(standard?.keyEquivalentModifierMask == .command)
    #expect(moved?.keyEquivalent == "x")
    #expect(moved?.keyEquivalentModifierMask == [.command, .shift])
  }

  @Test("Copy is offered only over a selection")
  func copyNeedsASelection() {
    _ = recorder()
    let bare = surfaceView(RecordingSurface())
    let selected = surfaceView(RecordingSurface(selection: "hello"))

    #expect(
      item(
        "Copy",
        in: ContextMenus.pane(
          laptopPane, surface: bare, machines: [], bindings: [], rename: { _ in }))?.isEnabled
        == false)
    #expect(
      item(
        "Copy",
        in: ContextMenus.pane(
          laptopPane, surface: selected, machines: [], bindings: [], rename: { _ in }))?.isEnabled
        == true)
  }

  @Test("a split from the menu splits the pane that was right-clicked")
  func aSplitNamesItsPane() {
    let recorder = recorder()
    let menu = ContextMenus.pane(
      laptopPane, surface: nil, machines: [], bindings: [], rename: { _ in })
    let mark = recorder.requests.count

    choose("Split Down", in: menu)

    let sent = recorder.sent(since: mark) {
      if case .splitPane = $0.payload { true } else { false }
    }
    #expect(sent.count == 1)
    #expect(sent.first?.splitPane.daemonID == "laptop")
    #expect(sent.first?.splitPane.paneID == "p1w3r07bsd")
    #expect(sent.first?.splitPane.side == "down")
    #expect(sent.first?.splitPane.newPaneDaemonID == "")
    #expect(sent.first?.splitPane.takeFocus == true)
  }

  /// The card's own item: a pane made where you want it, on whichever machine, without the CLI.
  @Test("picking a machine splits the pane onto that machine")
  func aSplitOntoAnotherMachine() {
    let recorder = recorder()
    let menu = ContextMenus.pane(
      laptopPane, surface: nil, machines: ["laptop", "devenv"], bindings: [], rename: { _ in })
    let mark = recorder.requests.count

    choose("devenv", in: item("Split on Machine", in: menu)?.submenu)

    let sent = recorder.sent(since: mark) {
      if case .splitPane = $0.payload { true } else { false }
    }
    #expect(sent.count == 1)
    #expect(sent.first?.splitPane.paneID == "p1w3r07bsd")
    #expect(sent.first?.splitPane.daemonID == "laptop")
    #expect(sent.first?.splitPane.newPaneDaemonID == "devenv")
    #expect(sent.first?.splitPane.side == "right")
  }

  @Test("zoom, move and close each name the pane that was right-clicked")
  func theRestNameTheirPane() {
    let recorder = recorder()
    let menu = ContextMenus.pane(
      laptopPane, surface: nil, machines: [], bindings: [], rename: { _ in })
    let mark = recorder.requests.count

    choose("Zoom Pane", in: menu)
    choose("Move Pane to New Tab", in: menu)
    choose("Close Pane", in: menu)

    let sent = recorder.requests.dropFirst(mark)
    #expect(
      sent.contains { $0.zoomPane.paneID == "p1w3r07bsd" && $0.zoomPane.daemonID == "laptop" })
    #expect(
      sent.contains {
        $0.arrangePane.paneID == "p1w3r07bsd" && $0.arrangePane.daemonID == "laptop"
          && $0.arrangePane.newTab
      })
    #expect(
      sent.contains { $0.closePane.paneID == "p1w3r07bsd" && $0.closePane.daemonID == "laptop" })
  }

  @Test("Copy Pane ID puts the id muster takes on the clipboard")
  func copyPaneID() {
    _ = recorder()
    let board = scratchClipboard()
    defer { board.releaseGlobally() }
    let view = surfaceView(RecordingSurface())
    view.pasteboard = board

    choose(
      "Copy Pane ID",
      in: ContextMenus.pane(
        laptopPane, surface: view, machines: [], bindings: [], rename: { _ in }))

    #expect(board.string(forType: .string) == "p1w3r07bsd")
  }

  @Test("Rename Pane… asks about the pane that was right-clicked")
  func renameNamesItsPane() {
    _ = recorder()
    var renamed: [PaneKey] = []
    let menu = ContextMenus.agentRow(
      laptopPane, onScreen: false, bindings: [], rename: { renamed.append($0) })

    choose("Rename Pane…", in: menu)

    #expect(renamed == [laptopPane])
  }

  /// The core refuses to close a pane no region shows, and a menu should not offer what it will
  /// refuse.
  @Test("an agent row offers Close Pane only for a pane on screen")
  func anAgentRowsMenu() {
    _ = recorder()
    let hidden = ContextMenus.agentRow(laptopPane, onScreen: false, bindings: [], rename: { _ in })
    let shown = ContextMenus.agentRow(laptopPane, onScreen: true, bindings: [], rename: { _ in })

    #expect(
      titles(hidden) == [
        "Rename Pane…", "Move Pane to New Tab", "-", "Copy Pane ID", "-", "Close Pane",
      ])
    #expect(item("Close Pane", in: hidden)?.isEnabled == false)
    #expect(item("Close Pane", in: shown)?.isEnabled == true)
  }

  @Test("a tab's menu names the tab, and grows a new tab from that tab's pane")
  func aTabsMenu() {
    let recorder = recorder()
    let board = scratchClipboard()
    defer { board.releaseGlobally() }
    let menu = ContextMenus.tab(
      "t1w3r07bsd", firstPane: laptopPane, machines: ["laptop", "devenv"], bindings: [],
      pasteboard: board, rename: { _ in })
    let mark = recorder.requests.count

    #expect(
      titles(menu) == [
        "New Tab", "New Tab on Machine", "-", "Rename Tab…", "Move Tab to Window", "-",
        "Copy Tab ID", "-", "Close Tab",
      ])
    choose("New Tab", in: menu)
    choose("devenv", in: item("New Tab on Machine", in: menu)?.submenu)
    choose("Close Tab", in: menu)
    choose("Copy Tab ID", in: menu)

    let sent = recorder.requests.dropFirst(mark)
    let created = sent.filter { if case .createTab = $0.payload { true } else { false } }
    #expect(created.map(\.createTab.paneID) == ["p1w3r07bsd", ""])
    #expect(created.map(\.createTab.daemonID) == ["laptop", "devenv"])
    #expect(sent.contains { $0.closeTab.tabID == "t1w3r07bsd" })
    #expect(board.string(forType: .string) == "t1w3r07bsd")
  }

  // --- the pane's surface ----------------------------------------------------------------

  @Test(
    "a right-click libghostty leaves to the host opens the pane's menu and asks for the keyboard")
  func aRightClickOpensTheMenu() {
    _ = recorder()
    let view = surfaceView(RecordingSurface())
    let menu = NSMenu()
    var clicks = 0
    view.onMenu = { menu }
    view.onClick = { clicks += 1 }

    #expect(view.menu(for: mouse(.rightMouseDown)) === menu)
    #expect(clicks == 1)
  }

  /// A program that asked for the mouse gets the click, as it would in Ghostty. AppKit asks for
  /// a menu only from `super.rightMouseDown`, so the surface consuming the press is the whole of
  /// the rule.
  @Test("a right-click the program asked for opens no menu")
  func aReportedRightClickIsTheProgramsAlone() {
    _ = recorder()
    let recording = RecordingSurface()
    recording.consumesButtons = true
    let view = surfaceView(recording)
    var asked = false
    view.onMenu = {
      asked = true
      return NSMenu()
    }

    view.rightMouseDown(with: mouse(.rightMouseDown))

    #expect(!asked)
    #expect(recording.buttons.map(\.number) == [1])
  }

  @Test("a ctrl-click opens the menu unless the program has the mouse")
  func aCtrlClick() {
    _ = recorder()
    let recording = RecordingSurface()
    let view = surfaceView(recording)
    view.onMenu = { NSMenu() }

    #expect(view.menu(for: mouse(.leftMouseDown, [.control])) != nil)
    #expect(view.menu(for: mouse(.leftMouseDown)) == nil)
    recording.mouseCaptured = true
    #expect(view.menu(for: mouse(.leftMouseDown, [.control])) == nil)
  }
}

private func titles(_ menu: NSMenu?) -> [String] {
  menu?.items.map { $0.isSeparatorItem ? "-" : $0.title } ?? []
}

private func item(_ title: String, in menu: NSMenu?) -> NSMenuItem? {
  menu?.items.first { $0.title == title }
}

@MainActor
private func choose(_ title: String, in menu: NSMenu?) {
  guard let menu, let index = menu.items.firstIndex(where: { $0.title == title }) else {
    Issue.record("no item called \(title) in \(titles(menu))")
    return
  }
  // A menu dispatches through `NSApp`, and without one the target is never reached.
  _ = NSApplication.shared
  menu.performActionForItem(at: index)
}

@MainActor
private func surfaceView(_ surface: RecordingSurface) -> SurfaceView {
  let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
  view.attach(surface, typeable: true)
  return view
}

private func scratchClipboard() -> NSPasteboard {
  let board = NSPasteboard(name: NSPasteboard.Name("muster.tests.menu.\(UUID())"))
  board.clearContents()
  return board
}

private func mouse(_ type: NSEvent.EventType, _ flags: NSEvent.ModifierFlags = []) -> NSEvent {
  NSEvent.mouseEvent(
    with: type, location: .zero, modifierFlags: flags, timestamp: 0, windowNumber: 0,
    context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
}
