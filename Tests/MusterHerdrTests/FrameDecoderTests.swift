import Foundation
import TestSupport
import Testing

@testable import MusterHerdr

// A driver. The cases live in corpus/conformance/frame-decoder.json, written as the lines
// herdr actually puts on the wire.

@Test("frame decoding")
func frameDecoderConformance() throws {
  let corpus = try Conformance.load("frame-decoder.json")

  let ran = corpus.run { given in
    var decoder = FrameDecoder()
    var events: [PaneStreamEvent] = []
    for chunk in chunks(from: given) {
      events += decoder.consume(chunk)
    }
    return .fields(["events": .array(events.map(describe))])
  }

  #expect(ran == corpus.cases.count)
  #expect(ran > 0)
}

/// The chunks to feed, in order.
///
/// `split: "bytes"` re-splits everything into single bytes - the worst case a repaint
/// split across reads can hit, and the one a decoder that trusts read boundaries fails.
private func chunks(from given: JSONValue) -> [Data] {
  let strings = given.strings("chunks")
  guard given["split"]?.stringValue == "bytes" else {
    return strings.map { Data($0.utf8) }
  }
  return strings.flatMap { $0.utf8 }.map { Data([$0]) }
}

private func describe(_ event: PaneStreamEvent) -> JSONValue {
  switch event {
  case .frame(let frame):
    .fields([
      "kind": "frame",
      "bytes_hex": .string(hex(frame.bytes)),
      "full": .bool(frame.isFull),
      "seq": .number(Double(frame.sequence)),
    ])
  case .closed(let reason):
    // Absent rather than null when there is no reason, so the corpus reads the way the
    // wire does: herdr omits the field rather than sending an empty one.
    .fields(["kind": "closed", "reason": reason.map(JSONValue.string)])
  }
}

private func hex(_ bytes: Data) -> String {
  bytes.map { String(format: "%02x", $0) }.joined()
}
