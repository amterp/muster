import AppKit
import Testing

@testable import MusterMac

// Where the panes go. Arithmetic rather than rendering, and worth its own file for that
// reason: a pane in the wrong place looks like a renderer problem and is a division, and the
// only cheap way to tell those apart is to check the division somewhere a GPU is not
// involved.
//
// Top-left origin throughout, matching the flipped container that renders it. `first` is the
// left child of a column split and the upper child of a row split, which is the one
// convention everything below depends on.

@Suite("pane geometry")
struct PaneTreeTests {
  private let bounds = CGRect(x: 0, y: 0, width: 1000, height: 600)

  private func leaf(_ id: String) -> PaneTree { .pane(.init(paneID: id, controlSocketPath: nil)) }

  @Test("one pane fills the region")
  func singlePaneFillsIt() {
    let frames = leaf("w1:p1").frames(in: bounds)

    #expect(frames.panes.count == 1)
    #expect(frames.panes[0].frame == bounds)
    // No split, so nothing to drag. A stray divider here would be an invisible strip
    // swallowing clicks along an edge.
    #expect(frames.dividers.isEmpty)
  }

  @Test("a column split puts the first pane on the left")
  func columnsRunLeftToRight() {
    let tree = PaneTree.split(axis: .columns, ratio: 0.5, first: leaf("a"), second: leaf("b"))

    let frames = tree.frames(in: bounds)

    #expect(frames.panes.map(\.paneID) == ["a", "b"])
    #expect(frames.panes[0].frame.minX == 0)
    #expect(frames.panes[1].frame.minX > frames.panes[0].frame.minX)
    // Both full height: a column split divides width and nothing else.
    #expect(frames.panes.allSatisfy { $0.frame.height == bounds.height })
  }

  @Test("a row split puts the first pane on top")
  func rowsRunTopToBottom() {
    // The inversion this catches renders every stack upside down, and looks deliberate.
    let tree = PaneTree.split(axis: .rows, ratio: 0.5, first: leaf("a"), second: leaf("b"))

    let frames = tree.frames(in: bounds)

    #expect(frames.panes[0].frame.minY == 0)
    #expect(frames.panes[1].frame.minY > frames.panes[0].frame.minY)
    #expect(frames.panes.allSatisfy { $0.frame.width == bounds.width })
  }

  @Test("the ratio is the first pane's share of what panes actually get")
  func ratioIsHonored() {
    let tree = PaneTree.split(axis: .columns, ratio: 0.25, first: leaf("a"), second: leaf("b"))

    let frames = tree.frames(in: bounds)

    let usable = bounds.width - PaneTree.dividerThickness
    #expect(frames.panes[0].frame.width == (usable * 0.25).rounded())
    // The divider comes out of the total, not out of one side, so the two panes and the
    // divider add up to the region. A window whose panes overlap by four points paints one
    // over the other and nobody can see which.
    let total = frames.panes.map(\.frame.width).reduce(0, +) + PaneTree.dividerThickness
    #expect(total == bounds.width)
  }

  @Test("nesting keeps every pane inside its parent")
  func nestedSplitsStayInside() {
    let tree = PaneTree.split(
      axis: .columns, ratio: 0.5,
      first: leaf("a"),
      second: .split(axis: .rows, ratio: 0.5, first: leaf("b"), second: leaf("c")))

    let frames = tree.frames(in: bounds)

    #expect(frames.panes.map(\.paneID) == ["a", "b", "c"])
    let (b, c) = (frames.panes[1].frame, frames.panes[2].frame)
    // b and c share the right half and split it vertically.
    #expect(b.minX == c.minX)
    #expect(b.width == c.width)
    #expect(b.maxY <= c.minY)
    #expect(frames.panes.allSatisfy { bounds.contains($0.frame) })
    // Two splits, two dividers - one per split node, not one per pane.
    #expect(frames.dividers.count == 2)
  }

  @Test("a divider is named by the turns down to it")
  func dividerPathsAddressTheTree() {
    // The path is how a drag names a divider to a daemon, and a divider has no id - so a
    // wrong path silently resizes a different split, which is the shape of bug that looks
    // like the daemon ignoring you.
    let tree = PaneTree.split(
      axis: .columns, ratio: 0.5,
      first: leaf("a"),
      second: .split(axis: .rows, ratio: 0.5, first: leaf("b"), second: leaf("c")))

    let frames = tree.frames(in: bounds)

    #expect(frames.dividers[0].path == [])
    #expect(frames.dividers[0].axis == .columns)
    // Reached by taking the second child once.
    #expect(frames.dividers[1].path == [true])
    #expect(frames.dividers[1].axis == .rows)
  }

  @Test("a pane is never squeezed to nothing")
  func degenerateRatiosStillLeaveARectangle() {
    // A daemon may legitimately publish a ratio of zero. A surface with no area is a PTY told
    // it has no columns, and a program that reads its window size gets an answer no terminal
    // ever gives.
    for ratio in [CGFloat(0), 1, -3, 42, .nan] {
      let tree = PaneTree.split(axis: .columns, ratio: ratio, first: leaf("a"), second: leaf("b"))

      let frames = tree.frames(in: bounds)

      #expect(frames.panes.allSatisfy { $0.frame.width >= PaneTree.minimumPaneSize })
      #expect(frames.panes.allSatisfy { $0.frame.height > 0 })
    }
  }

  @Test("a region too small to divide still produces two frames")
  func aTinyRegionDoesNotProduceNegativeFrames() {
    // Windows get dragged to nothing, and a negative width is a crash rather than a squeeze.
    let tree = PaneTree.split(axis: .rows, ratio: 0.5, first: leaf("a"), second: leaf("b"))

    let frames = tree.frames(in: CGRect(x: 0, y: 0, width: 3, height: 2))

    #expect(frames.panes.count == 2)
    #expect(frames.panes.allSatisfy { $0.frame.width >= 0 && $0.frame.height >= 0 })
  }

  @Test("dragging to a position asks for the ratio that puts the divider there")
  func aDragRoundTrips() {
    // The drag and the layout have to be inverses. If they are not, the divider crawls away
    // from the pointer while the user holds the mouse down - and every frame of that is a
    // round trip to the daemon that lands somewhere else again.
    let area = CGRect(x: 100, y: 0, width: 900, height: 600)
    for wanted in [CGFloat(0.2), 0.5, 0.75] {
      let ratio = PaneTree.ratio(
        at: CGPoint(
          x: area.minX + (area.width - PaneTree.dividerThickness) * wanted
            + PaneTree.dividerThickness / 2, y: 0),
        in: area, axis: .columns)

      #expect(abs(ratio - wanted) < 0.001)
    }
  }

  @Test("a drag outside the area asks for an end rather than for nonsense")
  func aDragIsClamped() {
    let area = CGRect(x: 0, y: 0, width: 500, height: 400)

    #expect(PaneTree.ratio(at: CGPoint(x: -900, y: 0), in: area, axis: .columns) == 0)
    #expect(PaneTree.ratio(at: CGPoint(x: 9000, y: 0), in: area, axis: .columns) == 1)
    #expect(PaneTree.ratio(at: CGPoint(x: 0, y: -900), in: area, axis: .rows) == 0)
  }
}

@Suite("the view the core publishes")
struct WindowContentsTests {
  @Test("a tree with no root is not a tab with no panes")
  func anAbsentTreeIsItsOwnAnswer() {
    // The distinction the whole message is built around: a shell told nil leaves its surfaces
    // alone, where one told "no panes" tears down surfaces that are about to be described.
    var region = Muster_ViewRegion()
    region.regionID = "r0"
    region.tabID = "w1:t1"
    var changed = Muster_ViewChanged()
    changed.regions = [region]

    let contents = WindowContents(changed)

    #expect(contents.regions[0].tree == nil)
    #expect(contents.regions[0].keyboardPane == nil)
    #expect(contents.focusedRegion == nil)
  }

  @Test("a pane with no socket says so rather than carrying an empty path")
  func anUnattachedPaneIsNil() {
    // A bridge spawned against an empty path dials nothing and the pane looks alive while
    // swallowing every keystroke, so the two must not be the same value here.
    var pane = Muster_ViewPane()
    pane.paneID = "w1:p1"
    var node = Muster_ViewNode()
    node.pane = pane

    guard case .pane(let leaf) = PaneTree(node) else {
      Issue.record("expected a leaf")
      return
    }
    #expect(leaf.controlSocketPath == nil)
  }

  @Test("a whole view arrives as regions, trees and a keyboard")
  func aViewCrossesIntact() {
    var first = Muster_ViewPane()
    first.paneID = "w1:p1"
    first.controlSocketPath = "/tmp/muster-1-0.sock"
    var second = Muster_ViewPane()
    second.paneID = "w1:p2"
    var split = Muster_ViewSplit()
    split.axis = "rows"
    split.ratio = 0.25
    split.first = .with { $0.pane = first }
    split.second = .with { $0.pane = second }
    var region = Muster_ViewRegion()
    region.regionID = "r0"
    region.daemonID = "devenv"
    region.tabID = "w1:t1"
    region.paneID = "w1:p2"
    region.root = .with { $0.split = split }
    var changed = Muster_ViewChanged()
    changed.regions = [region]
    changed.focusedRegion = "r0"

    let contents = WindowContents(changed)

    #expect(contents.keyboardPane == "w1:p2")
    // The daemon has to survive the crossing. A region that arrived without one would send
    // every click and every drag to whichever daemon the core was focused on, which is right
    // exactly as often as the window shows one daemon.
    #expect(contents.regions[0].daemon == "devenv")
    #expect(contents.regions[0].tree?.leaves.map(\.paneID) == ["w1:p1", "w1:p2"])
    #expect(contents.regions[0].tree?.leaves[0].controlSocketPath == "/tmp/muster-1-0.sock")
    guard case .split(let axis, let ratio, _, _) = contents.regions[0].tree else {
      Issue.record("expected a split")
      return
    }
    #expect(axis == .rows)
    #expect(abs(ratio - 0.25) < 0.001)
  }
}
