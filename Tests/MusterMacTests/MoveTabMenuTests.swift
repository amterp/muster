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
}
