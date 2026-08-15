import AppKit
import Testing

@testable import MusterMac

// What a region does with a view it is handed. No GPU, no libghostty, no daemon: the surface
// factory is a closure and this injects one that records rather than one that allocates.
//
// The property under test throughout is that identity is the pane id. The core publishes the
// whole arrangement every time, so a region that rebuilt what it was handed would tear down
// and respawn every surface and bridge in the window each time any pane in it changed - which
// is invisible in a screenshot and obvious the moment an agent's output flickers away.

/// Records the panes a region asked to have stood up, and what socket each was pointed at.
@MainActor
private final class Started {
  var panes: [String] = []
}

/// A region whose surfaces are recorded rather than allocated.
@MainActor
private func region(width: CGFloat = 800, height: CGFloat = 600) -> (RegionView, Started) {
  let started = Started()
  let view = RegionView(frame: NSRect(x: 0, y: 0, width: width, height: height)) {
    daemon, transport, chrome, socket in
    let machine = transport.map { "@\($0.sshHost)" } ?? ""
    started.panes.append("\(daemon)\(machine):\(chrome.paneID ?? "")@\(socket ?? "-")")
  }
  return (view, started)
}

private func leaf(_ id: String, socket: String? = "/tmp/\(0).sock") -> PaneTree {
  .pane(.init(paneID: id, controlSocketPath: socket))
}

private func contents(_ tree: PaneTree?, keyboard: String? = nil) -> WindowContents.Region {
  WindowContents.Region(
    id: "r0", daemon: "local", tab: "w1:t1", keyboardPane: keyboard, tree: tree, zoomed: false)
}

@Suite("a region renders a tree")
struct RegionViewTests {
  @MainActor
  @Test("a pane that survives a change keeps its surface")
  func survivingPanesAreNotRebuilt() {
    // The whole reason this class diffs rather than rebuilds. A split adds one pane; the
    // other one must not blink.
    let (view, started) = region()
    view.apply(contents(leaf("w1:p1", socket: "/tmp/a.sock")), focused: true)
    let before = view.chrome(for: "w1:p1")

    view.apply(
      contents(
        .split(
          axis: .columns, ratio: 0.5,
          first: leaf("w1:p1", socket: "/tmp/a.sock"),
          second: leaf("w1:p2", socket: "/tmp/b.sock"))),
      focused: true)

    #expect(view.chrome(for: "w1:p1") === before)
    #expect(started.panes == ["local:w1:p1@/tmp/a.sock", "local:w1:p2@/tmp/b.sock"])
  }

  @MainActor
  @Test("a pane the tree no longer names is let go")
  func closedPanesAreRemoved() {
    // Its surface goes with it, and so does the bridge that surface spawned. Left behind,
    // each closed pane costs a process and a socket for as long as the window is open.
    let (view, _) = region()
    view.apply(
      contents(.split(axis: .rows, ratio: 0.5, first: leaf("w1:p1"), second: leaf("w1:p2"))),
      focused: true)

    view.apply(contents(leaf("w1:p1")), focused: true)

    #expect(view.chrome(for: "w1:p2") == nil)
    #expect(view.paneIDs == ["w1:p1"])
    #expect(view.subviews.compactMap { ($0 as? PaneChrome)?.paneID } == ["w1:p1"])
  }

  @MainActor
  @Test("a pane whose socket moved gets a new bridge")
  func aChangedSocketRebuildsTheSurface() {
    // A bridge is spawned by its surface's command, so a pane whose socket changed is a pane
    // dialing a listener that is gone. It would keep painting and swallow every keystroke,
    // which is the failure that has cost this project the most time.
    let (view, started) = region()
    view.apply(contents(leaf("w1:p1", socket: nil)), focused: true)

    view.apply(contents(leaf("w1:p1", socket: "/tmp/opened.sock")), focused: true)

    #expect(started.panes == ["local:w1:p1@-", "local:w1:p1@/tmp/opened.sock"])
  }

  @MainActor
  @Test("a tree that has not arrived leaves what is on screen alone")
  func anAbsentTreeChangesNothing() {
    // herdr publishes a tab's panes and its tree separately, so this is an ordinary moment
    // rather than a failure. Tearing surfaces down for it is a flicker on every split.
    let (view, started) = region()
    view.apply(contents(leaf("w1:p1")), focused: true)

    view.apply(contents(nil), focused: true)

    #expect(view.paneIDs == ["w1:p1"])
    #expect(started.panes.count == 1)
  }

  @MainActor
  @Test("only the pane with the keyboard is marked as having it")
  func focusFollowsTheView() {
    let (view, _) = region()
    let tree = PaneTree.split(
      axis: .columns, ratio: 0.5, first: leaf("w1:p1"), second: leaf("w1:p2"))

    view.apply(contents(tree, keyboard: "w1:p2"), focused: true)

    #expect(view.chrome(for: "w1:p2")?.isFocused == true)
    #expect(view.chrome(for: "w1:p1")?.isFocused == false)
  }

  @MainActor
  @Test("a region that is not focused marks none of its panes")
  func anUnfocusedRegionShowsNoKeyboard() {
    // Two regions each drawing a focus ring is two windows' worth of "type here" on one
    // screen, and the user has no way to tell which one is lying.
    let (view, _) = region()

    view.apply(contents(leaf("w1:p1"), keyboard: "w1:p1"), focused: false)

    #expect(view.chrome(for: "w1:p1")?.isFocused == false)
  }

  @MainActor
  @Test("every pane and divider is laid out inside the region")
  func layoutFillsTheRegion() {
    let (view, _) = region(width: 900, height: 500)
    let tree = PaneTree.split(
      axis: .columns, ratio: 0.5,
      first: leaf("w1:p1"),
      second: .split(axis: .rows, ratio: 0.5, first: leaf("w1:p2"), second: leaf("w1:p3")))

    view.apply(contents(tree), focused: true)
    view.layoutSubtreeIfNeeded()

    let chromes = view.subviews.compactMap { $0 as? PaneChrome }
    #expect(chromes.count == 3)
    #expect(chromes.allSatisfy { view.bounds.contains($0.frame) })
    #expect(chromes.allSatisfy { $0.frame.width > 0 && $0.frame.height > 0 })
    // Three panes need two dividers, and every one of them is a grab handle: a missing one
    // is a split nobody can resize.
    #expect(view.subviews.filter { $0 is DividerView }.count == 2)
  }
}
