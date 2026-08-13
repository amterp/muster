import Darwin
import Foundation
import MusterCore

/// A request to a herdr daemon over its JSON socket.
///
/// One connection per request, because that is what the daemon does: it reads a single
/// newline-terminated line, answers with one line, and closes. Holding a connection open
/// buys nothing for anything Muster sends today.
///
/// Deliberately synchronous. The one caller serializes its own sends, because two routes
/// reach the same PTY and order between them is the whole correctness question; an async
/// client here would hand that problem back.
public final class HerdrClient: @unchecked Sendable {
  public let socketPath: String
  private let timeout: TimeInterval
  private var nextID = 0
  private let lock = NSLock()

  public enum Failure: Error, Equatable {
    case unreachable(errno: Int32)
    case timedOut
    case malformedResponse
    case daemon(code: String, message: String)
  }

  public init(socketPath: String, timeout: TimeInterval = 0.5) {
    self.socketPath = socketPath
    self.timeout = timeout
  }

  /// The socket a herdr daemon is listening on, by herdr's own rules.
  ///
  /// Ported from `src/session.rs` and `src/config/io.rs`, in precedence order:
  /// `HERDR_SOCKET_PATH` wins outright; otherwise a named `HERDR_SESSION` selects a
  /// per-session socket; otherwise the default session's. The base directory is
  /// `$XDG_CONFIG_HOME/herdr` or `~/.config/herdr`.
  ///
  /// A release herdr uses `herdr`; a debug build uses `herdr-dev`. Muster looks for the
  /// release directory, since that is what a person runs.
  public static func discoverSocketPath(
    environment: [String: String] = ProcessInfo.processInfo.environment
  ) -> String? {
    if let explicit = environment["HERDR_SOCKET_PATH"], !explicit.isEmpty { return explicit }

    let base: String
    if let xdg = environment["XDG_CONFIG_HOME"], !xdg.isEmpty {
      base = "\(xdg)/herdr"
    } else if let home = environment["HOME"], !home.isEmpty {
      base = "\(home)/.config/herdr"
    } else {
      return nil
    }

    // "default" is spelled by absence rather than by name, so it does not get a directory.
    if let session = environment["HERDR_SESSION"], !session.isEmpty, session != "default" {
      return "\(base)/sessions/\(session)/herdr.sock"
    }
    return "\(base)/herdr.sock"
  }

  /// Sends one request and returns the `result` object.
  public func request(method: String, params: [String: Any]) -> Result<[String: Any], Failure> {
    let id = nextRequestID()
    let envelope: [String: Any] = ["id": id, "method": method, "params": params]
    guard var payload = try? JSONSerialization.data(withJSONObject: envelope) else {
      return .failure(.malformedResponse)
    }
    // The newline is not decoration: the daemon reads exactly one line and blocks without
    // it (src/api/server.rs, read_initial_request_line).
    payload.append(contentsOf: [UInt8(ascii: "\n")])

    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return .failure(.unreachable(errno: errno)) }
    defer { close(fd) }

    // Both directions bounded, so a wedged daemon cannot take the keyboard with it.
    var limit = timeval(
      tv_sec: Int(timeout), tv_usec: Int32((timeout - floor(timeout)) * 1_000_000))
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &limit, socklen_t(MemoryLayout<timeval>.size))
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &limit, socklen_t(MemoryLayout<timeval>.size))

    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let pathBytes = Array(socketPath.utf8)
    guard pathBytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
      return .failure(.unreachable(errno: ENAMETOOLONG))
    }
    withUnsafeMutableBytes(of: &address.sun_path) { destination in
      pathBytes.withUnsafeBytes { destination.copyMemory(from: $0) }
    }
    let connected = withUnsafePointer(to: &address) {
      $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
      }
    }
    guard connected == 0 else { return .failure(.unreachable(errno: errno)) }

    guard write(fd, payload) else { return .failure(.timedOut) }
    _ = shutdown(fd, SHUT_WR)
    guard let line = readLine(fd) else { return .failure(.timedOut) }
    guard let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any] else {
      return .failure(.malformedResponse)
    }
    if let error = object["error"] as? [String: Any] {
      return .failure(
        .daemon(
          code: error["code"] as? String ?? "unknown",
          message: error["message"] as? String ?? ""))
    }
    return .success(object["result"] as? [String: Any] ?? [:])
  }

  private func nextRequestID() -> String {
    lock.lock()
    defer { lock.unlock() }
    nextID += 1
    return "muster:\(nextID)"
  }

  private func write(_ fd: Int32, _ data: Data) -> Bool {
    data.withUnsafeBytes { buffer in
      var sent = 0
      while sent < buffer.count {
        let n = Darwin.write(fd, buffer.baseAddress! + sent, buffer.count - sent)
        guard n > 0 else { return false }
        sent += n
      }
      return true
    }
  }

  /// Reads one newline-terminated response.
  private func readLine(_ fd: Int32) -> Data? {
    var out = Data()
    var byte: UInt8 = 0
    // A response is small and arrives in one or two reads; the cap only bounds a daemon
    // that has started saying something unbounded.
    while out.count < 1 << 20 {
      let n = read(fd, &byte, 1)
      guard n == 1 else { return out.isEmpty ? nil : out }
      if byte == UInt8(ascii: "\n") { return out }
      out.append(byte)
    }
    return out
  }
}
