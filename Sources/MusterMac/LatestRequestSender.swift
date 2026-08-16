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
  return .success(decoded)
}

/// Sends one request at a time and remembers only the newest, for the gestures that outrun
/// the seam.
///
/// Two things in this window produce requests faster than a daemon answers them. A drag
/// produces about a hundred mouse-moved events a second and each `set_split_ratio` is a ten
/// millisecond round trip; a find sends a needle per keystroke and each one reads a pane's
/// history back, which over an ssh-forwarded socket is tens of milliseconds. Sent
/// synchronously from the main thread, either gesture spends its whole duration inside the
/// seam with no time left to draw the thing being dragged or typed into (kan a_28h3eBJa2).
///
/// So a request is handed over rather than sent. One is in flight at a time and the latest is
/// remembered; when the answer arrives, whatever was asked for while it was out goes next.
/// **Nothing is lost at the end of a gesture** - the remembered request is always the last one
/// asked for, so the final one is always sent, which is what makes coalescing safe here rather
/// than merely cheap.
///
/// One type rather than two, though it began as two. A divider and a find bar want different
/// things from an answer - one only needs to hear a refusal, the other draws what came back -
/// and that difference was read as a reason to write the coalescing twice. It is not: the
/// difference is the `Answer` and what is done with it, and the part worth having once is the
/// interleaving of `pending` and `inFlight` across a thread hop, which is the part that is
/// subtle and the part a future change would otherwise have to be made in twice.
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
  ///
  /// `stale` says whether something newer was already waiting when this landed. A find bar
  /// drops those - drawing a count for a word somebody has typed past makes the counter
  /// flicker backwards - and a divider has nothing to draw either way.
  var onAnswer: (@MainActor (Result<Answer, Refused>, _ stale: Bool) -> Void)?

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
    pending = request
    sendPendingIfIdle()
  }

  /// Drops anything queued, for a gesture that is ending.
  ///
  /// A request already in flight still answers. What this prevents is something asked for
  /// just before the end going out after it.
  func cancel() {
    pending = nil
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
        // Stale means something newer is already waiting, so this answer is about a request
        // the person has already moved past.
        self.onAnswer?(answer, self.pending != nil)
        // Whatever was asked for while this was out, which is where the gesture is now.
        self.sendPendingIfIdle()
      }
    }
  }

}
