import Darwin
import Foundation
import Testing

@testable import MusterHerdr

/// The daemon-facing oracle `docs/testing.md` asks for: the exact bytes a message puts
/// on the wire. herdr parses these with serde, so a wrong key name is a silently ignored
/// command rather than an error anyone would see.

private func wire(_ message: ControlStreamMessage) throws -> [String: Any] {
  let data = message.wireFormat
  #expect(data.last == UInt8(ascii: "\n"), "herdr reads its stdin as newline-delimited JSON")
  return try #require(
    JSONSerialization.jsonObject(with: data.dropLast()) as? [String: Any])
}

@Test("input carries its bytes as base64 under the key herdr reads")
func inputMessageShape() throws {
  let object = try wire(.input(Array("\u{1b}[97u".utf8)))

  #expect(object["type"] as? String == "terminal.input")
  #expect(object["bytes"] as? String == "G1s5N3U=")
  // text and bytes together is an error herdr returns rather than a field it ignores.
  #expect(object["text"] == nil)
}

@Test("arbitrary bytes survive the round trip")
func inputSurvivesArbitraryBytes() throws {
  // Escape sequences and high bytes are the normal case here, not an edge one: this
  // channel exists to carry encoded keys, and herdr writes whatever arrives straight to
  // the pane's PTY.
  let bytes: [UInt8] = [0x1b, 0x00, 0xff, 0x0a, 0x7f, 0xc3, 0xa9]
  let object = try wire(.input(bytes))

  let encoded = try #require(object["bytes"] as? String)
  let decoded = try #require(Data(base64Encoded: encoded))
  #expect(Array(decoded) == bytes)
}

@Test("resize names its dimensions the way herdr does")
func resizeMessageShape() throws {
  let object = try wire(.resize(columns: 120, rows: 40))

  #expect(object["type"] as? String == "terminal.resize")
  #expect(object["cols"] as? Int == 120)
  #expect(object["rows"] as? Int == 40)
}

@Test("scroll goes out as an intent, not as bytes")
func scrollMessageShape() throws {
  let object = try wire(.scroll(direction: .up, lines: 3))

  #expect(object["type"] as? String == "terminal.scroll")
  #expect(object["direction"] as? String == "up")
  #expect(object["lines"] as? Int == 3)
}

@Test("the app's channel delivers whole messages to a connected bridge")
func channelDeliversMessages() throws {
  // The bridge is a subprocess in production; here it is a socket, because what is worth
  // testing is the framing and the connect race, not spawning.
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
