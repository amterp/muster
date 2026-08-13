import Darwin
import Foundation

/// The bridge's end of the app channel.
///
/// Dials the app and hands back whatever arrives, one newline-delimited message at a
/// time. It reads no meaning from the bytes: the app writes herdr's control-stream JSON
/// and this copies it onto herdr's stdin, so the message vocabulary stays in the
/// adapter and this stays a pipe.
final class ControlSocket {
  private let fd: Int32
  private let queue = DispatchQueue(label: "muster-bridge.control-socket")
  /// Held because a dispatch source stops when the last reference to it goes, which
  /// would make the pane stop accepting input the moment `relay` returned.
  private var source: DispatchSourceRead?

  /// Connects, or returns nil if the app is not there.
  ///
  /// Failing to connect is not fatal: a pane that renders is worth more than no pane,
  /// and the caller says so on stderr.
  init?(path: String) {
    fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return nil }

    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let pathBytes = Array(path.utf8)
    guard pathBytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
      close(fd)
      return nil
    }
    withUnsafeMutableBytes(of: &address.sun_path) { destination in
      pathBytes.withUnsafeBytes { destination.copyMemory(from: $0) }
    }

    let connected = withUnsafePointer(to: &address) {
      $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
      }
    }
    guard connected == 0 else {
      close(fd)
      return nil
    }
  }

  deinit {
    close(fd)
  }

  /// Calls `handle` with each whole line the app sends.
  ///
  /// Lines are reassembled here rather than passed on as they arrive, because herdr
  /// parses its stdin as newline-delimited JSON and half a message is a parse error that
  /// would desynchronize everything after it.
  func relay(_ handle: @escaping (Data) -> Void) {
    let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: queue)
    self.source = source
    var pending = Data()

    source.setEventHandler { [fd] in
      var buffer = [UInt8](repeating: 0, count: 4096)
      let count = read(fd, &buffer, buffer.count)
      guard count > 0 else {
        // The app is gone. The pane keeps rendering: sessions outlive the client.
        source.cancel()
        return
      }

      pending.append(contentsOf: buffer[0..<count])
      while let newline = pending.firstIndex(of: UInt8(ascii: "\n")) {
        let line = pending[pending.startIndex...newline]
        pending = pending[pending.index(after: newline)...]
        handle(Data(line))
      }
    }
    source.resume()
  }
}
