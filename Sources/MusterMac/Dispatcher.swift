import CMuster
import Foundation

/// The shell's way of reaching the core.
///
/// A protocol rather than the C function directly, so that the view tests - which are
/// about what the shell decides, not about what the core does with it - run against a
/// recorder instead of a live core. It also keeps the one call site that knows about
/// pointers in a file whose whole job is pointers.
public protocol Dispatcher: Sendable {
  /// Sends an encoded request and returns the encoded response.
  ///
  /// Empty means the core could not answer at all, which is a bug below this line rather
  /// than a refusal - a request the core understood and declined comes back as a response
  /// saying so.
  func dispatch(_ request: [UInt8]) -> [UInt8]
}

/// The real one: the seam symbol, and the copy that ends its buffer's life.
public struct CoreDispatcher: Dispatcher {
  public init() {}

  public func dispatch(_ request: [UInt8]) -> [UInt8] {
    var length = 0
    let response = request.withUnsafeBufferPointer { buffer in
      // baseAddress is nil for an empty array, which is exactly the null the core reads as
      // "no request bytes" - so this needs no special case.
      muster_dispatch(buffer.baseAddress, buffer.count, &length)
    }
    guard let response else { return [] }
    defer { muster_free(response, length) }
    return Array(UnsafeBufferPointer(start: response, count: length))
  }
}
