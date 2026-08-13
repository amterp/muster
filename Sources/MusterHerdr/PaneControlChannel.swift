import Darwin
import Foundation

/// The app's way of talking to a pane's bridge process.
///
/// The bridge exists because libghostty can only be fed by the command a surface spawns,
/// so the frame stream lives in a subprocess (`docs/observations/libghostty-9f9b8d1d.md`
/// section 2). That subprocess owns the pane's control stream, which is also the only
/// channel input can go out on - so the app needs a way to reach it.
///
/// A socket rather than the surface's own PTY. Writing input through the surface would
/// widen the renderer seam from "run a pane channel into it" to "and also carry
/// arbitrary bytes back out", and would tie Muster's input path to a renderer it intends
/// to be able to replace.
///
/// What crosses it is herdr's control-stream JSON, verbatim, so the bridge stays a relay
/// with no vocabulary of its own.
public final class PaneControlChannel {
  private let listener: Int32
  private let path: String
  private let queue = DispatchQueue(label: "muster.pane-control-channel")
  /// Written only on `queue`.
  private var client: Int32?

  public enum Failure: Error {
    case socketUnavailable(errno: Int32)
    case bindFailed(path: String, errno: Int32)
  }

  /// Opens a channel and returns the path the bridge should dial.
  ///
  /// The socket is bound before the surface is created, so the bridge cannot lose a race
  /// against its own listener.
  public init(path: String) throws {
    self.path = path

    listener = socket(AF_UNIX, SOCK_STREAM, 0)
    guard listener >= 0 else { throw Failure.socketUnavailable(errno: errno) }

    // A path left behind by a crashed run would make bind fail with EADDRINUSE; nothing
    // else can legitimately own this path, since it carries our own pid.
    unlink(path)

    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let pathBytes = Array(path.utf8)
    guard pathBytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
      close(listener)
      throw Failure.bindFailed(path: path, errno: ENAMETOOLONG)
    }
    withUnsafeMutableBytes(of: &address.sun_path) { destination in
      pathBytes.withUnsafeBytes { destination.copyMemory(from: $0) }
    }

    let bound = withUnsafePointer(to: &address) {
      $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        bind(listener, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
      }
    }
    guard bound == 0, listen(listener, 1) == 0 else {
      let code = errno
      close(listener)
      throw Failure.bindFailed(path: path, errno: code)
    }

    accept()
  }

  deinit {
    if let client { close(client) }
    close(listener)
    unlink(path)
  }

  /// The path to hand the bridge.
  public var socketPath: String { path }

  private func accept() {
    queue.async { [weak self] in
      guard let self else { return }
      let accepted = Darwin.accept(listener, nil, nil)
      guard accepted >= 0 else { return }

      // Without this, writing to a bridge that has died raises SIGPIPE and kills the
      // app - one pane's subprocess crashing would take every other pane's window with
      // it. macOS spells it as a socket option rather than a per-write flag.
      var on: Int32 = 1
      setsockopt(accepted, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))

      client = accepted
    }
  }

  /// Sends a message to the pane, if the bridge has connected.
  ///
  /// Returns whether it went out. A false is normal exactly once - in the moment between
  /// the surface starting and the bridge dialing back - and is a real problem after
  /// that, which is why the caller is told rather than the failure being swallowed here.
  @discardableResult
  public func send(_ message: ControlStreamMessage) -> Bool {
    let data = message.wireFormat
    return queue.sync {
      guard let client else { return false }
      return data.withUnsafeBytes { buffer -> Bool in
        var sent = 0
        while sent < buffer.count {
          let wrote = Darwin.send(client, buffer.baseAddress! + sent, buffer.count - sent, 0)
          guard wrote > 0 else { return false }
          sent += wrote
        }
        return true
      }
    }
  }
}
