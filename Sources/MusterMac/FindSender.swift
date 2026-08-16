import Foundation

/// Sends needles to the core, one at a time, without blocking the typing.
///
/// A find is a round trip to a daemon - the core reads the pane's history back before it can
/// match anything - and it happens once per keystroke. On a laptop's own daemon that is a
/// millisecond; over an ssh-forwarded socket to a devenv it is tens of them, and typing into a
/// field that stalls for a tenth of a second per character is a field nobody types into.
///
/// The coalescing that makes that bearable is `LatestRequestSender`, which the divider drag
/// uses too. What is here is only what a find does differently: it has an answer to draw, and
/// an answer about a needle somebody has already typed past is a counter that flickers
/// backwards, so a stale one is dropped rather than shown.
@MainActor
public final class FindSender {
  private let sender: LatestRequestSender<Core.Findings>

  /// What the core answered, on the main thread. Only the latest answer is drawn.
  public var onFindings: (@MainActor (Core.Findings) -> Void)?

  public init(dispatcher: Dispatcher = Core.dispatcher) {
    sender = LatestRequestSender(
      what: "find", queue: "muster.find", dispatcher: dispatcher,
      read: { response in
        readResponse(response).flatMap { decoded in
          guard case .findings(let findings) = decoded.payload else {
            return .failure(Refused("the core answered a find with something other than findings"))
          }
          return .success(Core.read(findings))
        }
      })
    sender.onAnswer = { [weak self] answer, stale in
      guard let self, !stale else { return }
      // A refusal still draws, as nothing found: a bar that kept its last count would be
      // reporting matches for a needle the core never looked for.
      self.onFindings?((try? answer.get()) ?? .none)
    }
  }

  /// Asks what a needle finds, and returns without waiting.
  public func send(needle: String) {
    var find = Muster_Find()
    find.needle = needle
    var request = Muster_Request()
    request.find = find
    sender.send(request)
  }

  /// Stops anything queued, for a bar that is closing.
  ///
  /// A request already in flight still answers, and its answer reaches a bar nobody is
  /// looking at, which costs nothing. What this prevents is a needle typed just before the
  /// close going out after it.
  public func cancel() {
    sender.cancel()
  }
}
