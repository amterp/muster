import Foundation

/// Why the core would not do something, in the words it used.
///
/// Prose rather than a code, because that is what the seam carries: "the core answers every
/// request, including the ones it refuses ... so a refusal is a `Failure` carrying prose
/// written for whoever finds it in a log, not an error code to branch on"
/// (`docs/architecture.md`, the shell/core seam). A type only because Swift's `Result` wants
/// an `Error` on the failing side.
struct Refused: Error {
  let reason: String
  init(_ reason: String) { self.reason = reason }
}

/// What the core said, or why there is nothing to read.
///
/// Free and `nonisolated` because it runs on a sender's background queue, before anything
/// hops back to the main thread. The two failures it folds together want different words even
/// though both end here: a `Failure` payload is the core refusing and says why, while an empty
/// response is the core gone.
func readResponse(_ response: [UInt8]) -> Result<Muster_Response, Refused> {
  guard !response.isEmpty, let decoded = try? Muster_Response(serializedBytes: response) else {
    return .failure(Refused("the core did not answer"))
  }
  if case .failure(let failure) = decoded.payload {
    return .failure(Refused(failure.reason))
  }
  // Not a refusal, and nothing here branches on the difference: every sender on this path wants
  // an answer to act on, and a change its daemon never answered about gives it none. The reason
  // says which it was.
  if case .unanswered(let unanswered) = decoded.payload {
    return .failure(Refused(unanswered.reason))
  }
  return .success(decoded)
}

/// Sends one request at a time and remembers only the newest, for the gestures that outrun
/// the seam.
///
/// Two things in this window produce requests faster than a daemon answers them: a divider
/// drag and a window being moved or resized. Each produces about a hundred events a second,
/// and a round trip is measured in milliseconds. Sent synchronously from the main thread, the
/// gesture spends its whole duration inside the seam with no time left to draw the thing being
/// dragged (kan a_28h3eBJa2).
///
/// So a request is handed over rather than sent. One is in flight at a time and the latest is
/// remembered; when the answer arrives, whatever was asked for while it was out goes next.
/// **Nothing is lost at the end of a gesture** - the remembered request is always the last one
/// asked for, so the final one is always sent, which is what makes coalescing safe here rather
/// than merely cheap.
///
/// One type rather than one per gesture, because the part worth having once is the interleaving
/// of `pending` and `inFlight` across a thread hop, which is the part that is subtle and the
/// part a future change would otherwise have to be made in twice.
@MainActor
final class LatestRequestSender<Answer: Sendable> {
  /// What was asked for while a request was out, and not yet sent.
  private var pending: Muster_Request?
  private var inFlight = false

  /// Off the main thread and serial. Serial is not what limits concurrency - `inFlight` does -
  /// but it keeps two requests from ever being in the seam at once if that stops being true.
  private let queue: DispatchQueue

  /// The core this sender talks to, taken once.
  ///
  /// `Core.dispatcher` is a mutable global declared `nonisolated(unsafe)` on the grounds that
  /// this seam has no concurrency. Taking it here keeps that true - the background side never
  /// touches the global - and binds a sender to one core for its life, which is what a caller
  /// wants: a request that left before the core was swapped should still be answered by the
  /// core it was addressed to.
  private let dispatcher: Dispatcher

  /// What this sender calls itself in a log line, so a refusal names the request that drew it.
  private let what: String

  /// Reads the answer out of what the core said, on the queue rather than the main thread.
  ///
  /// `nonisolated` and taken at construction because it runs off the main thread; acting on
  /// what it returns is the caller's business and happens on the main thread below.
  private let read: @Sendable ([UInt8]) -> Result<Answer, Refused>

  /// What came back, on the main thread.
  var onAnswer: (@MainActor (Result<Answer, Refused>) -> Void)?

  init(
    what: String,
    queue label: String,
    dispatcher: Dispatcher = Core.dispatcher,
    read: @Sendable @escaping ([UInt8]) -> Result<Answer, Refused>
  ) {
    self.what = what
    self.queue = DispatchQueue(label: label)
    self.dispatcher = dispatcher
    self.read = read
  }

  /// Asks for something, and returns without waiting for it.
  func send(_ request: Muster_Request) {
    // Named now, on the main thread, while whatever window is sending is still the one speaking.
    var request = request
    Core.address(&request)
    pending = request
    sendPendingIfIdle()
  }

  private func sendPendingIfIdle() {
    guard !inFlight, let next = pending else { return }
    pending = nil
    inFlight = true

    guard let encoded = try? next.serializedBytes() as [UInt8] else {
      inFlight = false
      Core.error(
        "\(what).encode.failed",
        [
          "impact": "this request never reached the core, so the window is showing the "
            + "answer to an older one",
          "check": "a bug in Muster's request building rather than anything a user did",
        ])
      return
    }

    let dispatcher = self.dispatcher
    let read = self.read
    queue.async {
      let answer = read(dispatcher.dispatch(encoded))
      Task { @MainActor in
        self.inFlight = false
        if case .failure(let refused) = answer {
          Core.error("core.refused", ["request": self.what, "reason": refused.reason])
        }
        self.onAnswer?(answer)
        // Whatever was asked for while this was out, which is where the gesture is now.
        self.sendPendingIfIdle()
      }
    }
  }

}
