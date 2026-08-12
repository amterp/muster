import Foundation
import Testing

@testable import MusterHerdr

/// Builds a frame envelope the way herdr does, so the cases below read as stream content
/// rather than as JSON assembly.
private func frameLine(_ ansi: String, full: Bool = false, seq: Int = 1) -> Data {
  let payload = Data(ansi.utf8).base64EncodedString()
  let json = """
    {"type":"terminal.frame","seq":\(seq),"encoding":"ansi","full":\(full),\
    "width":80,"height":24,"bytes":"\(payload)"}
    """
  return Data((json + "\n").utf8)
}

@Test("a whole frame decodes to its ANSI payload")
func decodesOneFrame() {
  var decoder = FrameDecoder()
  let events = decoder.consume(frameLine("\u{1b}[2Jhello", full: true, seq: 7))

  #expect(
    events == [.frame(PaneFrame(bytes: Data("\u{1b}[2Jhello".utf8), isFull: true, sequence: 7))])
}

@Test("a frame split across reads is held until it is whole")
func reassemblesSplitFrame() {
  let line = frameLine("\u{1b}[1;1Hx")
  var decoder = FrameDecoder()

  // Byte at a time is the worst case a 35 KB repaint can hit, and the one that catches a
  // decoder that trusts read boundaries.
  var events: [PaneStreamEvent] = []
  for byte in line {
    events += decoder.consume(Data([byte]))
  }

  #expect(events.count == 1)
  #expect(
    events.first == .frame(PaneFrame(bytes: Data("\u{1b}[1;1Hx".utf8), isFull: false, sequence: 1)))
}

@Test("several frames in one read all come back, in order")
func decodesBatchedFrames() {
  var decoder = FrameDecoder()
  var chunk = frameLine("a", seq: 1)
  chunk.append(frameLine("b", seq: 2))
  chunk.append(frameLine("c", seq: 3))

  let events = decoder.consume(chunk)

  #expect(events.count == 3)
  #expect(events.map { if case .frame(let f) = $0 { f.sequence } else { -1 } } == [1, 2, 3])
}

@Test("a trailing partial line yields nothing until its newline arrives")
func withholdsPartialLine() {
  var decoder = FrameDecoder()
  let line = frameLine("z")
  let split = line.index(line.startIndex, offsetBy: line.count - 5)

  #expect(decoder.consume(line[line.startIndex..<split]).isEmpty)
  #expect(decoder.consume(line[split...]).count == 1)
}

@Test("a close ends the stream and carries its reason")
func decodesClose() {
  var decoder = FrameDecoder()
  let events = decoder.consume(
    Data("{\"type\":\"terminal.closed\",\"reason\":\"detached\"}\n".utf8))

  #expect(events == [.closed(reason: "detached")])
}

@Test("a message we do not know is skipped, not fatal")
func skipsUnknownAndMalformed() {
  var decoder = FrameDecoder()
  var chunk = Data("{\"type\":\"terminal.something_new\"}\n".utf8)
  chunk.append(Data("not json at all\n".utf8))
  chunk.append(frameLine("still here", seq: 9))

  let events = decoder.consume(chunk)

  // herdr ships weekly and may add message types; dropping the pane's whole stream over
  // one unknown line is worse than ignoring it.
  #expect(events.count == 1)
  #expect(
    events.first == .frame(PaneFrame(bytes: Data("still here".utf8), isFull: false, sequence: 9)))
}
