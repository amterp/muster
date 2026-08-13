import Foundation
import Testing

@testable import MusterCore

// The log is the thing that gets read when something has already gone wrong, so the ways
// it can quietly fail are expensive: a line that will not parse, a field that leaks what
// someone typed, a level that hides the record that mattered.

@Test("a record reads as JSON, identity first and fields sorted")
func recordEncoding() throws {
  let record = LogRecord(
    time: Date(timeIntervalSince1970: 0), level: .warn, process: "app", pid: 42,
    event: "input.dropped", fields: ["socket": "/tmp/x.sock", "impact": "nothing arrived"])

  let line = JSONLinesSink.encode(record)

  // Identity before payload: the four things you scan a log with come first.
  #expect(
    line.hasPrefix(
      "{\"time\":\"1970-01-01T00:00:00.000Z\",\"level\":\"warn\",\"process\":\"app\",\"pid\":42,"
        + "\"event\":\"input.dropped\""))
  // Sorted, so the same code twice produces the same bytes.
  #expect(line.hasSuffix("\"impact\":\"nothing arrived\",\"socket\":\"/tmp/x.sock\"}"))

  let parsed = try JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any]
  #expect(parsed?["event"] as? String == "input.dropped")
}

@Test("a payload cannot break out of its line")
func quotingIsSafe() throws {
  // Terminal bytes are exactly what this log carries, and they are full of escapes,
  // quotes and control characters. One unescaped newline splits a record into two
  // unparseable halves.
  let nasty = "a\"b\\c\nd\te\u{1b}[97u\u{0}"
  let record = LogRecord(
    time: Date(timeIntervalSince1970: 0), level: .debug, process: "app", pid: 1,
    event: "input.key", fields: ["encoded": nasty])

  let line = JSONLinesSink.encode(record)

  #expect(!line.contains("\n"))
  let parsed = try JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any]
  #expect(parsed?["encoded"] as? String == nasty)
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
