import AppKit

/// Sends divider positions to the core, one at a time, without blocking the drag.
///
/// A drag produces about a hundred mouse-moved events a second, and every one of them used to
/// become a `set_split_ratio` sent synchronously from `mouseDragged` - which runs on the main
/// thread. Each send is a round trip to a daemon, measured at ten milliseconds, so the window
/// spent the whole gesture inside the seam and had no time left to draw the thing being
/// dragged. Measured on a real drag: a hundred requests a second, a ten-millisecond median gap
/// between them, and the gap was the round trip rather than the mouse (kan a_28h3eBJa2).
///
/// The coalescing that fixes that is `LatestRequestSender`, which a moving window uses too.
/// What is here is only what a divider does differently, which is nearly nothing: there is no
/// answer to draw, so a success is dropped, and a refusal is logged and answered with the
/// platform's beep, as a refused chord held down is. The divider goes back to where the daemons
/// say it is, and without the beep that reads as the drag being ignored.
///
@MainActor
public final class SplitRatioSender {
  private let sender: LatestRequestSender<Void>

  /// Called on the main thread once a position has been answered, for a test that has to know
  /// when the round trip it started is over. Nothing in the app sets it: a drag is finished
  /// when the pointer stops, not when the last answer lands.
  public var onAnswered: (@MainActor () -> Void)?

  /// What a refused position sounds like. Once for a run of refusals rather than for each: a
  /// drag that cannot move its divider sends a position for every few points it travels.
  public var onRefused: @MainActor () -> Void = { NSSound.beep() }
  private var refusing = false

  public init(dispatcher: Dispatcher = Core.dispatcher) {
    sender = LatestRequestSender(
      what: "set_split_ratio", queue: "muster.split-ratio", dispatcher: dispatcher,
      read: { response in
        readResponse(response).map { _ in () }
      })
    sender.onAnswer = { [weak self] answer in
      guard let self else { return }
      let refused = if case .failure = answer { true } else { false }
      if refused && !refusing { onRefused() }
      refusing = refused
      onAnswered?()
    }
  }

  /// Asks for a divider position, and returns without waiting for it. The path is the turns
  /// down the tab's tree as this window draws it, whatever machines the divider sits between:
  /// the core finds which of them it divides.
  public func send(tab: String, path: [Bool], ratio: CGFloat) {
    var set = Muster_SetSplitRatio()
    set.tabID = tab
    set.path = path
    set.ratio = Float(ratio)
    var request = Muster_Request()
    request.setSplitRatio = set
    sender.send(request)
  }
}
