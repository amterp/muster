import AppKit
import Testing

@testable import MusterMac

// The arrangement over regions is the one part of the layout Muster owns outright: no daemon
// knows the other one exists, so nothing upstream will ever say how a laptop and a devenv
// divide a window. Until now it was a division by the count, inside `layout`, where nothing
// could reach it.

@Suite("regions divide the window by weight")
struct RegionStripTests {
  private let bounds = CGRect(x: 0, y: 0, width: 1000, height: 600)

  @Test("one region takes the whole window and has no line beside it")
  func oneRegionIsTheWholeWindow() {
    let placed = RegionStripLayout.place(weights: [1], in: bounds)

    #expect(placed.count == 1)
    #expect(placed[0].frame == bounds)
    // Nothing to its right to divide against. A divider here would be a handle that moves
    // nothing, sitting where the terminal should be.
    #expect(placed[0].divider == nil)
  }

  @Test("equal weights are equal shares, which is what a window starts as")
  func equalWeightsSplitEvenly() {
    let placed = RegionStripLayout.place(weights: [1, 1], in: bounds)
    let line = RegionStripLayout.dividerThickness

    #expect(placed[0].frame.width == placed[1].frame.width)
    #expect(placed[0].frame.width == (bounds.width - line) / 2)
    // The line is between them, not over either.
    #expect(placed[0].divider?.minX == placed[0].frame.maxX)
    #expect(placed[1].frame.minX == placed[0].frame.maxX + line)
  }

  @Test("weights are relative, not absolute")
  func weightsAreShares() {
    // Three to one is three quarters, whatever the numbers happen to be. A weight is a
    // share of the sum, so nothing has to normalise before sending one.
    let small = RegionStripLayout.place(weights: [3, 1], in: bounds)
    let large = RegionStripLayout.place(weights: [300, 100], in: bounds)

    #expect(small[0].frame == large[0].frame)
    #expect(small[1].frame == large[1].frame)
    #expect(small[0].frame.width == (bounds.width - RegionStripLayout.dividerThickness) * 0.75)
  }

  @Test("every region and every line fits inside the window")
  func nothingOverflows() {
    let placed = RegionStripLayout.place(weights: [2, 1, 1], in: bounds)
    let widths = placed.map(\.frame.width).reduce(0, +)
    let lines = CGFloat(placed.filter { $0.divider != nil }.count)

    #expect(widths + lines * RegionStripLayout.dividerThickness == bounds.width)
    #expect(placed.last?.frame.maxX == bounds.maxX)
    #expect(placed.last?.divider == nil)
  }

  @Test("a drag is measured against the pair, not the window")
  func theAreaIsThePair() {
    // What makes a drag local. The ratio a divider reports is that pair's share, so moving
    // one line leaves every other region exactly where it was - which is what dragging looks
    // like to the person doing it.
    let placed = RegionStripLayout.place(weights: [1, 1, 1], in: bounds)
    let area = placed[0].area

    #expect(area?.minX == placed[0].frame.minX)
    #expect(area?.maxX == placed[1].frame.maxX)
    #expect(area?.maxX != bounds.maxX)
  }

  @Test("a weight that is not a number is read as an equal share")
  func nonsenseDoesNotCollapseAWindow() {
    // A weight arrives across the seam and is not this window's to validate. A NaN would
    // poison the total and lose every region; a zero would collapse one to nothing with no
    // divider left to drag it back.
    let placed = RegionStripLayout.place(weights: [.nan, 1], in: bounds)

    #expect(placed[0].frame.width == placed[1].frame.width)
    #expect(placed.allSatisfy { $0.frame.width.isFinite })
    #expect(RegionStripLayout.place(weights: [0, 1], in: bounds)[0].frame.width > 0)
  }

  @Test("a window too narrow for the lines gives no region a negative width")
  func aCrampedWindowStaysSane() {
    // Frames are handed to real views, and a negative width is a crash or a mess rather than
    // a small pane.
    let cramped = CGRect(x: 0, y: 0, width: 2, height: 100)
    let placed = RegionStripLayout.place(weights: [1, 1], in: cramped)

    #expect(placed.allSatisfy { $0.frame.width >= 0 })
  }

  @Test("no regions is no placements, not a division by zero")
  func nothingIsNothing() {
    #expect(RegionStripLayout.place(weights: [], in: bounds).isEmpty)
  }
}
