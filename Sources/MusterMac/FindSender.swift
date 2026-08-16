import Foundation

/// Sends needles to the core, one at a time, without blocking the typing.
///
/// A find is a round trip to a daemon - the core reads the pane's history back before it can
/// match anything - and it happens once per keystroke. On a laptop's own daemon that is a
/// millisecond; over an ssh-forwarded socket to a devenv it is tens of them, and typing into a
/// field that stalls for a tenth of a second per character is a field nobody types into.
///
/// So the needle is handed over rather than sent. One request is in flight at a time and the
/// latest needle is remembered; when the answer arrives, whatever was typed while it was out
/// goes next. Nothing is lost at the end: the remembered needle is always the last one typed,
/// so the final one is always asked about.
///
/// The same shape as `SplitRatioSender`, and deliberately not the same object. That one asks
/// for a change and is finished when the daemon has made it; this one asks a question and the
/// answer is drawn - so the sender has to carry a result back to the main thread, and merging
/// the two would mean giving the divider drag a callback it has no use for.
@MainActor
public final class FindSender {
  /// The needle typed while a request was out, and not yet asked about.
  private var pending: String?
  private var inFlight = false

  /// Off the main thread and serial, for the same reason `SplitRatioSender` is: `inFlight` is
  /// what limits concurrency, and this keeps two needles from ever being in the seam at once
  /// if that ever stops being true.
  private let queue = DispatchQueue(label: "muster.find")

  private let dispatcher: Dispatcher

  /// What the core answered, on the main thread. Only the latest answer is drawn - an answer
  /// to a needle somebody has already typed past is a counter that flickers backwards.
  public var onFindings: (@MainActor (Core.Findings) -> Void)?

  public init(dispatcher: Dispatcher = Core.dispatcher) {
    self.dispatcher = dispatcher
  }

  /// Asks what a needle finds, and returns without waiting.
  public func send(needle: String) {
    pending = needle
    sendPendingIfIdle()
  }

  /// Stops anything queued, for a bar that is closing.
  ///
  /// A request already in flight still answers, and its answer reaches a bar nobody is
  /// looking at, which costs nothing. What this prevents is a needle typed just before the
  /// close going out after it.
  public func cancel() {
    pending = nil
  }

  private func sendPendingIfIdle() {
    guard !inFlight, let needle = pending else { return }
    pending = nil
    inFlight = true

    var find = Muster_Find()
    find.needle = needle
    var request = Muster_Request()
    request.find = find
    guard let encoded = try? request.serializedBytes() as [UInt8] else {
      inFlight = false
      Core.error(
        "find.encode.failed",
        [
          "impact": "this needle never reached the core, so the find bar is showing counts "
            + "for something older than what is typed in it",
          "check": "a bug in Muster's request building rather than anything a user did",
        ])
      return
    }

    let dispatcher = self.dispatcher
    queue.async {
      let response = dispatcher.dispatch(encoded)
      let answer = Self.findings(in: response)
      Task { @MainActor in
        self.inFlight = false
        switch answer {
        case .success(let findings):
          // Dropped if something newer is already waiting: that answer is about a needle the
          // person has typed past, and drawing it would count the wrong word for a moment.
          if self.pending == nil { self.onFindings?(findings) }
        case .failure(let reason):
          Core.error("core.refused", ["request": "find", "reason": reason])
          if self.pending == nil { self.onFindings?(.none) }
        }
        // Whatever was typed while this was out, which is what is in the field now.
        self.sendPendingIfIdle()
      }
    }
  }

  private enum Answer {
    case success(Core.Findings)
    case failure(String)
  }

  /// What the core said, read on the queue and acted on from the main thread - because acting
  /// goes back through the seam, and one place entering it is the whole point of this type.
  private nonisolated static func findings(in response: [UInt8]) -> Answer {
    guard !response.isEmpty, let decoded = try? Muster_Response(serializedBytes: response) else {
      return .failure("the core did not answer, so nothing was searched")
    }
    switch decoded.payload {
    case .findings(let findings): return .success(Core.read(findings))
    case .failure(let failure): return .failure(failure.reason)
    default:
      return .failure("the core answered a find with something other than findings")
    }
  }
}
