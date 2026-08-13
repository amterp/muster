import Foundation
import TestSupport
import Testing

@testable import MusterCore

// The log is the thing that gets read when something has already gone wrong, so the ways
// it can quietly fail are expensive: a line that will not parse, a field that leaks what
// someone typed, a level that hides the record that mattered.

@Test("log record encoding")
func logRecordConformance() throws {
  let corpus = try Conformance.load("log-record.json")

  let ran = corpus.run { given in
    guard let level = LogLevel(rawValue: given["level"]?.stringValue ?? "") else {
      throw CaseError("`level` is missing or not a level")
    }
    var fields: [String: String] = [:]
    if case .object(let raw)? = given["fields"] {
      for (name, value) in raw { fields[name] = value.stringValue ?? "" }
    }
    let record = LogRecord(
      // Fixed rather than read from the case: the timestamp's rendering is Foundation's,
      // and a corpus that pinned it would be testing a date formatter in each language
      // instead of this encoder. Every case uses the epoch, so the text is constant.
      time: Date(timeIntervalSince1970: 0),
      mono: UInt64(given["mono"]?.intValue ?? 0), level: level,
      process: given["process"]?.stringValue ?? "", pid: Int32(given["pid"]?.intValue ?? 0),
      event: given["event"]?.stringValue ?? "", fields: fields)

    return .fields(["line": .string(JSONLinesSink.encode(record))])
  }

  #expect(ran == corpus.cases.count)
  #expect(ran > 0)
}

@Test("every encoded record is parseable JSON")
func encodedRecordsParse() throws {
  // The corpus pins the exact bytes; this pins that those bytes mean what they look like.
  // A rendering that agreed with itself in both languages and parsed in neither would
  // satisfy the cases above and still be useless.
  let corpus = try Conformance.load("log-record.json")
  for testCase in corpus.cases {
    let line = try #require(testCase.expect["line"]?.stringValue)
    #expect(
      (try? JSONSerialization.jsonObject(with: Data(line.utf8))) != nil,
      "\(testCase.name): the expected line is not JSON")
  }
}

private struct CaseError: Error, CustomStringConvertible {
  let description: String
  init(_ description: String) { self.description = description }
}

@Test("the monotonic reading advances, and resolves finer than the wall clock")
func monotonicClockResolvesTheHopsWeMeasure() {
  // The point of carrying a second clock: `time` is milliseconds, and the hops the perf
  // harness times are tenths of one. A clock that cannot see them makes the log useless
  // as a perf oracle while still looking like it works.
  let start = MonotonicClock.now()
  #expect(start > 0)

  var later = MonotonicClock.now()
  while later == start { later = MonotonicClock.now() }

  #expect(later > start)
  // Two readings taken back to back are microseconds apart at most. If this clock only
  // ticked per millisecond, the loop above would have spun for one.
  #expect(later - start < 1_000_000)
}

@Test("levels order from noise to alarm")
func levelOrdering() {
  #expect(LogLevel.trace < .debug)
  #expect(LogLevel.debug < .info)
  #expect(LogLevel.info < .warn)
  #expect(LogLevel.warn < .error)
}

@Test("the sink appends whole lines that survive several writers")
func sinkAppends() throws {
  let path = FileManager.default.temporaryDirectory
    .appendingPathComponent("muster-log-test-\(UUID().uuidString).jsonl").path
  defer { try? FileManager.default.removeItem(atPath: path) }

  // Two sinks on one path is the real arrangement - the app and a bridge - so the test
  // opens it the way the product does rather than through one handle.
  let app = try #require(JSONLinesSink(path: path))
  let bridge = try #require(JSONLinesSink(path: path))
  for index in 0..<50 {
    app.write(
      LogRecord(
        time: Date(), level: .info, process: "app", pid: 1, event: "e", fields: ["i": "\(index)"]))
    bridge.write(
      LogRecord(
        time: Date(), level: .info, process: "bridge", pid: 2, event: "e",
        fields: ["i": "\(index)"]))
  }

  let lines = try String(contentsOfFile: path, encoding: .utf8)
    .split(separator: "\n", omittingEmptySubsequences: true)
  #expect(lines.count == 100)
  // Every line parses: nothing was interleaved into a torn record.
  for line in lines {
    #expect((try? JSONSerialization.jsonObject(with: Data(line.utf8))) != nil)
  }
}

@Test("an unwritable path turns logging off rather than failing the process")
func unwritablePathIsNotFatal() {
  #expect(JSONLinesSink(path: "/muster-does-not-exist/nope.jsonl") == nil)
}
