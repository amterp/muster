import Testing

@testable import MusterMac

@Suite("moving a tab to another window")
struct MoveTabMenuTests {
  /// An open window is named by the pid `muster window` prints, and a closed one by its name -
  /// the same handles `muster tab move --window` takes, so what the menu says is what a script
  /// would type.
  @Test("a window is named the way the CLI names it")
  func aWindowIsNamedTheWayTheCLINamesIt() {
    let open = Core.OtherWindow(name: "window-2", pid: 4321, tabs: 3)
    let closed = Core.OtherWindow(name: "window-3", pid: 0, tabs: 1)

    #expect(open.title == "Window 4321 (3 tabs)")
    #expect(closed.title == "window-3 (closed, 1 tab)")
  }

  /// A tab row goes between windows and a pane row stays within one. A tab dropped into its own
  /// window's list is going nowhere, and a pane dragged in from another window would need that
  /// window to let go of it, which moving a pane between windows does not do yet.
  @Test("a tab is dropped from another window, and a pane only from this one")
  func whatMayBeDroppedWhere() {
    #expect(SidebarModel.acceptsTab(fromThisWindow: false))
    #expect(!SidebarModel.acceptsTab(fromThisWindow: true))
    #expect(SidebarModel.acceptsPane(fromThisWindow: true))
    #expect(!SidebarModel.acceptsPane(fromThisWindow: false))
  }
}
