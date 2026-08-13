import Foundation
import MusterHerdr
import MusterVT
import TestSupport
import Testing

/// What a user would see, computed by the engine that would show it to them.
///
/// These replay bytes a real herdr daemon actually sent (`corpus/herdr-0.8.0/frames/`)
/// through libghostty-vt and snapshot the resulting grid. The oracle is the screen, not
/// a substring of the stream and not a pixel: `docs/testing.md`, "assert what the user
/// sees".

/// The corpus, resolved from this file rather than from a bundle - test resources would
/// mean copying the transcripts, and a copy is a thing that can go stale.
private func corpusFile(_ path: String) -> URL {
  URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .appendingPathComponent("corpus/herdr-0.8.0/\(path)")
}

@Test("a recorded attach repaint renders the pane's screen")
func attachFrameRendersScreen() throws {
  // 35 KB of absolute positioning and per-cell SGR: what herdr sends a client that has
  // just attached, and the one frame every surface is guaranteed to see.
  let ansi = try Data(contentsOf: corpusFile("frames/frame-001-attach.ansi"))

  let terminal = try Terminal(columns: 80, rows: 24)
  terminal.write(ansi)

  try Snapshot.expect(
    GridSnapshot.render(terminal.viewport(columns: 80, rows: 24)),
    named: "attach-repaint.txt")
}

@Test("the whole recorded stream converges on the same screen")
func fullStreamRendersScreen() throws {
  // Through the real decoder, so the test covers the path a pane actually takes:
  // envelopes off the wire, frames out, bytes into a terminal.
  let stream = try Data(contentsOf: corpusFile("frames/frames.ndjson"))
  var decoder = FrameDecoder()
  let terminal = try Terminal(columns: 80, rows: 24)

  var frames = 0
  for event in decoder.consume(stream) {
    guard case .frame(let frame) = event else { continue }
    terminal.write(frame.bytes)
    frames += 1
  }

  #expect(frames > 0, "the corpus stream carried no frames, so nothing was rendered")
  try Snapshot.expect(
    GridSnapshot.render(terminal.viewport(columns: 80, rows: 24)),
    named: "recorded-stream.txt")
}

@Test("a full repaint replaces the screen rather than layering onto it")
func fullRepaintReplacesScreen() throws {
  // A surface attaching to a live pane starts mid-stream and must not inherit whatever
  // was on it (architecture.md, "the shell owns nothing"). herdr's repaint clears
  // first; this pins that we get the clear, not a merge.
  let terminal = try Terminal(columns: 20, rows: 3)
  terminal.write(Array("stale text everywhere".utf8))

  let repaint = try Data(contentsOf: corpusFile("frames/frame-001-attach.ansi"))
  terminal.write(repaint)

  let rows = terminal.viewport(columns: 20, rows: 3).rows
  #expect(!rows.contains { $0.text.contains("stale") })
}
