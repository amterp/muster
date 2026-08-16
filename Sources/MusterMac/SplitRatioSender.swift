import Foundation

/// Sends divider positions to the core, one at a time, without blocking the drag.
///
/// A drag produces about a hundred mouse-moved events a second, and every one of them used to
/// become a `set_split_ratio` sent synchronously from `mouseDragged` - which runs on the main
/// thread. Each send is a round trip to a daemon, measured at ten milliseconds, so the window
/// spent the whole gesture inside the seam and had no time left to draw the thing being
/// dragged. Measured on a real drag: a hundred requests a second, a ten-millisecond median gap
/// between them, and the gap was the round trip rather than the mouse (kan a_28h3eBJa2).
///
/// So the position is handed over rather than sent. One request is in flight at a time and the
/// latest position is remembered; when the request returns, the position that arrived while it
/// was out goes next. The drag then runs at whatever the round trip allows instead of queueing
/// behind itself, and `mouseDragged` returns immediately either way. Nothing is lost at the end
/// of a gesture: the remembered position is always the last one asked for, so the final one is
/// always sent.
///
/// Deliberately only this request. The other drag in the window moves a region boundary, which
/// is Muster's own composition and never reaches a daemon - there is nothing there to wait for.
@MainActor
public final class SplitRatioSender {
  /// The position asked for while a request was out, and not yet sent.
  private var pending: Muster_SetSplitRatio?
  private var inFlight = false

  /// Off the main thread and serial. Serial is not what limits concurrency here - `inFlight`
  /// does - but it keeps two positions from ever being in the seam at once if that changes.
  private let queue = DispatchQueue(label: "muster.split-ratio")

  /// The core this sender talks to, taken once.
  ///
  /// `Core.dispatcher` is a mutable global declared `nonisolated(unsafe)` on the grounds that
  /// this seam has no concurrency. Taking it here keeps that true - the background side never
  /// touches the global - and binds a sender to one core for its life, which is what a region
  /// wants: a request that left before the core was swapped should still be answered by the
  /// core it was addressed to.
  private let dispatcher: Dispatcher

  /// Called on the main thread once a position has been answered, for a test that has to know
  /// when the round trip it started is over. Nothing in the app sets it: a drag is finished
  /// when the pointer stops, not when the last answer lands.
  public var onAnswered: (@MainActor () -> Void)?

  public init(dispatcher: Dispatcher = Core.dispatcher) {
    self.dispatcher = dispatcher
  }

  /// Asks for a divider position, and returns without waiting for it.
  public func send(daemonID: String, tab: String, path: [Bool], ratio: CGFloat) {
    var set = Muster_SetSplitRatio()
    set.daemonID = daemonID
    set.tabID = tab
    set.path = path
    set.ratio = Float(ratio)
    pending = set
    sendPendingIfIdle()
  }

  private func sendPendingIfIdle() {
    guard !inFlight, let next = pending else { return }
    pending = nil
    inFlight = true

    var request = Muster_Request()
    request.setSplitRatio = next
    guard let encoded = try? request.serializedBytes() as [UInt8] else {
      inFlight = false
      Core.error(
        "divider.encode.failed",
        [
          "impact": "this divider position never reached the core, and the drag will look "
            + "stuck until the next mouse-moved event",
          "check": "a bug in Muster's request building rather than anything a user did",
        ])
      return
    }

    let dispatcher = self.dispatcher
    queue.async {
      let response = dispatcher.dispatch(encoded)
      let refusal = Self.refusal(in: response)
      Task { @MainActor in
        self.inFlight = false
        if let refusal {
          Core.error("core.refused", ["request": "set_split_ratio", "reason": refusal])
        }
        // Whatever arrived while this was out, which is the position the pointer is at now.
        self.sendPendingIfIdle()
        self.onAnswered?()
      }
    }
  }

  /// Why the core would not move the divider, if it would not.
  ///
  /// Read on the queue and reported on the main thread, because reporting goes back through
  /// the seam - and the whole point of this type is that the seam is entered from one place.
  private static func refusal(in response: [UInt8]) -> String? {
    guard !response.isEmpty, let decoded = try? Muster_Response(serializedBytes: response) else {
      return "the core did not answer, so the divider stayed where it was"
    }
    guard case .failure(let failure) = decoded.payload else { return nil }
    return failure.reason
  }
}
