import Foundation
import MusterHerdr
import MusterVT
import TestSupport
import Testing

/// Two renderers, one screen, and they have to agree.
///
/// The grid harness claims that replaying a pane's frames tells us what the user sees.
/// That is a claim about herdr's frames *and* about the reading code in `MusterVT`, and
/// a snapshot cannot check either - it only pins whatever we produced on the day.
///
/// So the corpus records both halves of the same moment: the frames herdr sent, and
/// herdr's own text of the screen those frames describe (`pane.read`). Any disagreement
/// is a real defect in one of the two, which is exactly the check `docs/testing.md`
/// wants standing between the fast tier and reality.

private func fidelityFile(_ name: String) -> URL {
  URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .appendingPathComponent("corpus/herdr-0.8.0/frame-fidelity/\(name)")
}

private func replayFidelityFrames() throws -> Grid {
  let stream = try Data(contentsOf: fidelityFile("frames.ndjson"))
  var decoder = FrameDecoder()
  let terminal = try Terminal(columns: 80, rows: 24)
  for event in decoder.consume(stream) {
    guard case .frame(let frame) = event else { continue }
    terminal.write(frame.bytes)
  }
  return terminal.viewport(columns: 80, rows: 24)
}

@Test("replayed frames reproduce the daemon's own screen")
func replayedFramesMatchDaemonScreen() throws {
  let grid = try replayFidelityFrames()
  let daemonScreen = try String(contentsOf: fidelityFile("herdr-screen.txt"), encoding: .utf8)

  // herdr returns only the rows it has written; the grid is always the full viewport.
  // Trailing blank rows are agreement, not difference.
  let expected = daemonScreen.components(separatedBy: "\n").map(trimTrailing)
  let actual = grid.rows.map { trimTrailing($0.text) }

  for (index, want) in expected.enumerated() where !want.isEmpty {
    #expect(
      index < actual.count && actual[index] == want,
      """
      Row \(index) differs between the two renderers.
        herdr:      \(want.debugDescription)
        libghostty: \((index < actual.count ? actual[index] : "<past end of grid>").debugDescription)
      """)
  }
}

@Test("wide characters occupy two cells and leave a spacer")
func wideCharactersClaimTwoCells() throws {
  let grid = try replayFidelityFrames()
  let row = try #require(grid.rows.first { $0.text.hasPrefix("wide: ") })

  // A reader that treated 你 as one cell would still produce the right text, and would
  // put every following column in the wrong place. The spacer is what says otherwise.
  let han = try #require(row.cells.firstIndex { $0.text == "你" })
  #expect(row.cells[han].width == .wide)
  #expect(row.cells[han + 1].width == .spacerTail)
  #expect(row.cells[han + 1].text.isEmpty)
}

@Test("a combining mark stays in the cell it belongs to")
func combiningMarkStaysWithItsCell() throws {
  let grid = try replayFidelityFrames()
  let row = try #require(grid.rows.first { $0.text.hasPrefix("combining: ") })

  // The payload writes the same letter twice: once as e + U+0301, once as U+00E9. A
  // reader that returned only a cell's first codepoint would drop the mark from one of
  // them and render "e and é".
  let text = row.text
  #expect(text.contains("é and é"), "combining mark lost: \(text.debugDescription)")
}

private func trimTrailing(_ text: String) -> String {
  var text = text
  while text.last == " " { text.removeLast() }
  return text
}
