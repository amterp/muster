import AppKit
import MusterRenderer
import Testing

@testable import MusterMac

// What libghostty makes of a double click Muster drives rather than a person.
//
// The one claim behind re-placing a word selection that cannot be argued from Muster's own
// code. A word selection is libghostty's, its cells cannot be read back, and the only lever an
// embedder has is the mouse (`observations/libghostty-9f9b8d1d.md` section 12) - so putting a
// word selection back after a scroll means driving two presses at the cell the text moved to
// and trusting the click counter to land on two rather than three. Whether it does is a
// question about libghostty, and this asks libghostty.
//
// One test with a real surface, and the only one here that has one: no window and no GPU
// needed, but a real runtime and a real command behind a pty, and two runtimes in one process
// hang. It is worth what it costs, because the alternative is a feature that takes a whole line
// every so often and nobody knows why.

/// Two rows of known words, and the column each one starts at.
///
/// Two rows because the case that matters is vertical: a pane scrolls by whole rows, so a
/// re-drive lands the same distance sideways and one or more rows away.
private let firstRow = "alpha beta gamma delta"
private let secondRow = "epsilon zeta eta theta"
/// Inside `beta` on the first row and inside `zeta` on the second, so one column reaches a
/// different word on each.
private let sharedColumn = 8
private let gamma = 11
private let alpha = 0

@Suite("a double click Muster drives")
struct ClickRedriveTests {
  @MainActor
  @Test("it selects the word under it, unless it lands where the last one did")
  func aRedriveSelectsTheWordUnlessItRepeats() async throws {
    let pane = try await RealPane.showing(firstRow, and: secondRow)

    // A cell in a column nothing has just pressed. libghostty compares a press against the last
    // one and starts the count over when it is more than a cell's width away, so the second
    // press of the pair is the first repeat and takes the word.
    #expect(pane.doubleClick(column: gamma) == "gamma")
    #expect(pane.doubleClick(column: sharedColumn) == "beta")

    // The same column one row down, which is what a scroll actually produces. A row is taller
    // than the cell's width the comparison uses, so this resets too.
    #expect(pane.doubleClick(column: sharedColumn, row: pane.row + 1) == "zeta")

    // The same cell again, which is the case the feature must never produce: nothing resets the
    // count, so these two presses are the third and the line comes back instead of the word.
    #expect(pane.doubleClick(column: sharedColumn, row: pane.row + 1) == secondRow)

    // The other way the count can end: expired rather than moved. Only the interval between the
    // pair has to be short, and that one is under Muster's control.
    let expired = UInt64(NSEvent.doubleClickInterval * 2 * 1_000_000_000)
    try? await Task.sleep(nanoseconds: expired)
    #expect(pane.doubleClick(column: alpha) == "alpha")
  }
}

/// A pane with a real libghostty surface behind it, showing known text.
@MainActor
private struct RealPane {
  let surface: Surface
  /// The row the first line landed on, which is below whatever the shell printed before it.
  let row: Int
  private let cell: (width: Double, height: Double)

  /// One runtime for the process. `ghostty_init` and the app handle are global to it, and
  /// `Renderer.current` is where the wakeup callback looks - a second one is a hang.
  private static var runtime: Renderer?

  /// Starts a surface, waits for the text to reach the screen, and says which row it is on.
  static func showing(_ first: String, and second: String) async throws -> RealPane {
    let view = NSView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
    let renderer =
      try runtime ?? Renderer(configPath: NSTemporaryDirectory() + "muster-redrive.conf")
    runtime = renderer
    Renderer.current = renderer
    // Short-lived on purpose: a command outliving the test is a process left on somebody's
    // machine, and the surface only has to stay alive while this runs.
    let surface = try renderer.makeSurface(
      in: view, command: "printf '\(first)\\n\(second)\\n'; sleep 5")
    surface.setSize(width: 800, height: 600)
    let size = try #require(surface.cellPixelSize)
    let cell = (width: Double(size.width), height: Double(size.height))
    let pane = RealPane(surface: surface, row: 0, cell: cell)

    // Which row the text lands on is not something to assume - the shell prints its own banner
    // first. Dragged rather than read, because a drag is the only way to get the grid back
    // through this seam, which is the same limitation the feature works around.
    for _ in 0..<200 {
      try? await Task.sleep(nanoseconds: 25_000_000)
      for row in 0..<12 where pane.line(row: row) == first {
        return RealPane(surface: surface, row: row, cell: cell)
      }
    }
    Issue.record("the command's text never reached the surface, so there was nothing to select")
    throw CancellationError()
  }

  /// The text of one row, read the only way this seam allows: by dragging across it.
  func line(row: Int) -> String? {
    surface.select(
      .dragged(
        from: NSPoint(x: 0.5 * cell.width, y: (Double(row) + 0.5) * cell.height),
        to: NSPoint(x: 60 * cell.width, y: (Double(row) + 0.5) * cell.height)))
    return surface.selectedText?.trimmingCharacters(in: .whitespacesAndNewlines)
  }

  /// Drives two presses at a cell, back to back, and says what libghostty selected.
  func doubleClick(column: Int, row: Int? = nil) -> String? {
    let at = NSPoint(
      x: (Double(column) + 0.5) * cell.width,
      y: (Double(row ?? self.row) + 0.5) * cell.height)
    surface.mouseMoved(to: at, modifiers: [])
    for _ in 0..<2 {
      surface.leftMouse(pressed: true, modifiers: [])
      surface.leftMouse(pressed: false, modifiers: [])
    }
    return surface.selectedText?.trimmingCharacters(in: .whitespacesAndNewlines)
  }
}
