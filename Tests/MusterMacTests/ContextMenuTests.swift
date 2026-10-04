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

@Suite("right-click menus", .ownsTheSeam)
@MainActor
struct ContextMenuTests {
  @Test("a pane's menu offers Ghostty's items and Muster's own, in that order")
  func aPanesMenu() {
    _ = recorder()
    let menu = ContextMenus.pane(
      laptopPane, surface: nil, machines: ["laptop"], rename: { _ in })

    #expect(
      titles(menu) == [
        "Copy", "Paste", "-", "Split Right", "Split Down", "Split Left", "Split Up", "-",
        "Zoom Pane", "Move Pane to New Tab", "Rename Pane…", "Compact Agent", "-",
        "Copy Pane ID", "-", "Close Pane",
      ])
  }

  /// One machine has nothing to choose between, and most windows never attach a second.
  @Test("the machine submenu appears once a second machine is attached, listing every one")
  func theMachineSubmenu() {
    _ = recorder()
    let one = ContextMenus.pane(
      laptopPane, surface: nil, machines: ["laptop"], rename: { _ in })
    let two = ContextMenus.pane(
      laptopPane, surface: nil, machines: ["laptop", "devenv"], rename: { _ in })

    #expect(!titles(one).contains("Split on Machine"))
    #expect(titles(item("Split on Machine", in: two)?.submenu) == ["laptop", "devenv"])
  }

  /// Every item acts on what was right-clicked and every chord on the pane with the keyboard,
  /// so a chord shown here would describe a different pane whenever those two differ.
  @Test("no item shows a chord, not even Copy and Paste")
  func noChords() {
    _ = recorder()
    let menu = ContextMenus.pane(
      laptopPane, surface: nil, machines: ["laptop", "devenv"], rename: { _ in })
    let row = ContextMenus.agentRow(laptopPane, onScreen: true, rename: { _ in })
    let tab = ContextMenus.tab("t1", firstPane: laptopPane, machines: [], rename: { _ in })

    for item in [menu, row, tab].flatMap(\.items)
      + (item("Split on Machine", in: menu)?.submenu?
        .items ?? [])
    {
      #expect(item.keyEquivalent == "", "\(item.title) shows a chord")
    }
  }

  /// A menu bar pick and a right-click pick of the same action act on different panes, so the run
  /// log has to tell them apart, and say which pane or tab the right-click was on.
  @Test("a pick is logged as a right-click, naming what was right-clicked")
  func aPickIsLoggedWithItsSubject() {
    let recorder = recorder()
    let mark = recorder.requests.count

    choose(
      "Zoom Pane", in: ContextMenus.pane(laptopPane, surface: nil, machines: [], rename: { _ in }))
    choose("Close Tab", in: ContextMenus.tab("t1", firstPane: nil, machines: [], rename: { _ in }))

    let records = recorder.sent(since: mark) {
      if case .logRecord(let record) = $0.payload {
        record.event == BoundAction.event
      } else {
        false
      }
    }.map(\.logRecord.fields)
    #expect(records.count == 2)
    #expect(records.first?["action"] == "zoom")
    #expect(records.first?["source"] == "context_menu")
    #expect(records.first?["daemon"] == "laptop")
    #expect(records.first?["pane"] == "p1w3r07bsd")
    #expect(records.last?["action"] == "close_tab")
    #expect(records.last?["source"] == "context_menu")
    #expect(records.last?["tab"] == "t1")
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
          laptopPane, surface: bare, machines: [], rename: { _ in }))?.isEnabled
        == false)
    #expect(
      item(
        "Copy",
        in: ContextMenus.pane(
          laptopPane, surface: selected, machines: [], rename: { _ in }))?.isEnabled
        == true)
  }

  @Test("a split from the menu splits the pane that was right-clicked")
  func aSplitNamesItsPane() {
    let recorder = recorder()
    let menu = ContextMenus.pane(
      laptopPane, surface: nil, machines: [], rename: { _ in })
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
      laptopPane, surface: nil, machines: ["laptop", "devenv"], rename: { _ in })
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
      laptopPane, surface: nil, machines: [], rename: { _ in })
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
        laptopPane, surface: view, machines: [], rename: { _ in }))

    #expect(board.string(forType: .string) == "p1w3r07bsd")
  }

  @Test("Rename Pane… asks about the pane that was right-clicked")
  func renameNamesItsPane() {
    _ = recorder()
    var renamed: [PaneKey] = []
    let menu = ContextMenus.agentRow(
      laptopPane, onScreen: false, rename: { renamed.append($0) })

    choose("Rename Pane…", in: menu)

    #expect(renamed == [laptopPane])
  }

  /// Typing a compact line is the daemon's, and it can type none for a pane with no agent or an
  /// agent whose manifest gives none: a menu should not offer what it will refuse.
  @Test("Compact Agent is offered only where the pane's daemon says its agent compacts")
  func compactNeedsAnAgentThatCompacts() {
    _ = recorder()
    let menus: [(Bool) -> NSMenu] = [
      {
        ContextMenus.pane(laptopPane, surface: nil, machines: [], canCompact: $0, rename: { _ in })
      },
      { ContextMenus.agentRow(laptopPane, onScreen: false, canCompact: $0, rename: { _ in }) },
    ]
    for menu in menus {
      #expect(item("Compact Agent", in: menu(false))?.isEnabled == false)
      #expect(item("Compact Agent", in: menu(true))?.isEnabled == true)
    }
  }

  @Test("Compact Agent compacts the pane that was right-clicked, keeping nothing in particular")
  func compactNamesItsPane() {
    let recorder = recorder()
    let menus = [
      ContextMenus.pane(laptopPane, surface: nil, machines: [], canCompact: true, rename: { _ in }),
      ContextMenus.agentRow(laptopPane, onScreen: false, canCompact: true, rename: { _ in }),
    ]
    let mark = recorder.requests.count

    for menu in menus { choose("Compact Agent", in: menu) }

    let sent = recorder.sent(since: mark) {
      if case .compactPane = $0.payload { true } else { false }
    }.map(\.compactPane)
    #expect(sent.count == 2)
    #expect(sent.allSatisfy { $0.daemonID == "laptop" && $0.paneID == "p1w3r07bsd" })
    #expect(sent.allSatisfy { $0.focus.isEmpty })
  }

  /// The core refuses to close a pane no region shows, and a menu should not offer what it will
  /// refuse.
  @Test("an agent row offers Close Pane only for a pane on screen")
  func anAgentRowsMenu() {
    _ = recorder()
    let hidden = ContextMenus.agentRow(laptopPane, onScreen: false, rename: { _ in })
    let shown = ContextMenus.agentRow(laptopPane, onScreen: true, rename: { _ in })

    #expect(
      titles(hidden) == [
        "Rename Pane…", "Compact Agent", "Move Pane to New Tab", "-", "Copy Pane ID", "-",
        "Close Pane",
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
      "t1w3r07bsd", firstPane: laptopPane, machines: ["laptop", "devenv"],
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
