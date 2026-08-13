import Darwin
import Foundation
import TestSupport
import Testing

@testable import MusterHerdr

// A driver, plus the one case that cannot be data: whether the app's channel actually
// delivers a whole message to a bridge that has connected. That needs a socket and a
// race, not a table.

@Test("control stream wire format")
func controlStreamConformance() throws {
  let corpus = try Conformance.load("control-stream-message.json")

  let ran = corpus.run { given in
    let data = try message(from: given).wireFormat

    // Asserted for every case rather than in one of them: herdr reads its stdin as
    // newline-delimited JSON and blocks forever without the terminator, which looks
    // exactly like a pane that ignores the keyboard.
    let terminated = data.last == UInt8(ascii: "\n")
    guard let object = try? JSONSerialization.jsonObject(with: data.dropLast()) else {
      throw CaseError("the message is not JSON once its newline is removed")
    }

    guard case .object(var fields) = JSONValue(object) else {
      throw CaseError("the message is not a JSON object")
    }
    fields["newline_terminated"] = .bool(terminated)
    return .object(fields)
  }

  #expect(ran == corpus.cases.count)
  #expect(ran > 0)
}

private func message(from given: JSONValue) throws -> ControlStreamMessage {
  switch given["intent"]?.stringValue {
  case "input":
    guard let bytes = bytes(fromHex: given["bytes_hex"]?.stringValue ?? "") else {
      throw CaseError("`bytes_hex` is missing or not hex")
    }
    return .input(bytes)
  case "resize":
    guard let columns = given["columns"]?.intValue, let rows = given["rows"]?.intValue else {
      throw CaseError("resize needs `columns` and `rows`")
    }
    return .resize(columns: UInt16(columns), rows: UInt16(rows))
  case "scroll":
    guard let name = given["direction"]?.stringValue,
      let direction = ControlStreamMessage.ScrollDirection(rawValue: name),
      let lines = given["lines"]?.intValue
    else {
      throw CaseError("scroll needs a known `direction` and `lines`")
    }
    return .scroll(direction: direction, lines: UInt16(lines))
  case let other:
    throw CaseError("unknown intent \(other ?? "nil")")
  }
}

private func bytes(fromHex hex: String) -> [UInt8]? {
  guard hex.count.isMultiple(of: 2) else { return nil }
  var out: [UInt8] = []
  var index = hex.startIndex
  while index < hex.endIndex {
    let next = hex.index(index, offsetBy: 2)
    guard let byte = UInt8(hex[index..<next], radix: 16) else { return nil }
    out.append(byte)
    index = next
  }
  return out
}

private struct CaseError: Error, CustomStringConvertible {
  let description: String
  init(_ description: String) { self.description = description }
}

@Test("the app's channel delivers whole messages to a connected bridge")
func channelDeliversMessages() throws {
  // Stays native: what is worth testing here is framing across a real socket and the
  // connect race, neither of which a case file can express.
  let path = FileManager.default.temporaryDirectory
    .appendingPathComponent("muster-test-\(getpid())-\(UInt32.random(in: 0...999_999)).sock")
    .path
  let channel = try PaneControlChannel(path: path)
  defer { unlink(path) }

  let client = socket(AF_UNIX, SOCK_STREAM, 0)
  #expect(client >= 0)
  defer { close(client) }

  var address = sockaddr_un()
  address.sun_family = sa_family_t(AF_UNIX)
  withUnsafeMutableBytes(of: &address.sun_path) { destination in
    Array(path.utf8).withUnsafeBytes { destination.copyMemory(from: $0) }
  }
  let connected = withUnsafePointer(to: &address) {
    $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
      connect(client, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
    }
  }
  #expect(connected == 0)

  // The accept happens on the channel's own queue, so a send can legitimately lose the
  // race. That is the one moment `send` is allowed to report false, and a pane that
  // swallowed keys forever would look identical - so the test waits rather than sleeping
  // and hoping.
  var delivered = false
  for _ in 0..<200 where !delivered {
    delivered = channel.send(.input(Array("hello".utf8)))
    if !delivered { usleep(5_000) }
  }
  #expect(delivered, "the bridge connected but the channel never accepted it")

  var buffer = [UInt8](repeating: 0, count: 1024)
  let count = read(client, &buffer, buffer.count)
  #expect(count > 0)

  let line = try #require(String(bytes: buffer[0..<max(0, count)], encoding: .utf8))
  #expect(line.hasSuffix("\n"))
  #expect(line.contains("\"type\":\"terminal.input\"") || line.contains("\"terminal.input\""))
}
