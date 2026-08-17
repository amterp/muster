import Foundation

/// Tells the core where the window has settled, without holding up the gesture that moved it.
///
/// Dragging a window by its title bar posts `windowDidMove` at the display's refresh rate, and
/// a live resize does the same - so a report per event is a hundred a second, each one ending in
/// the core rewriting `window.toml`. Sent synchronously from the main thread that is the drag
/// spending its whole duration inside the seam, which is the problem `SplitRatioSender` was
/// built for and this is the same answer: one request in flight, the latest remembered, and the
/// last position always sent.
///
/// The last one mattering is what makes coalescing safe rather than merely cheap. Where the
/// window ended up is the only position worth writing down, and it is always the one that goes.
///
/// Reported as it settles rather than at quit, because quitting is not how this is usually lost:
/// the card behind it was raised from an app that was killed outright, and a crash or a reboot
/// costs the same thing.
@MainActor
public final class WindowFrameSender {
  private let sender: LatestRequestSender<Void>

  /// Called on the main thread once a frame has been answered, for a test that has to know when
  /// the round trip it started is over. Nothing in the app sets it.
  public var onAnswered: (@MainActor () -> Void)?

  public init(dispatcher: Dispatcher = Core.dispatcher) {
    sender = LatestRequestSender(
      what: "set_window_frame", queue: "muster.window-frame", dispatcher: dispatcher,
      read: { response in
        readResponse(response).map { _ in () }
      })
    sender.onAnswer = { [weak self] _, _ in self?.onAnswered?() }
  }

  /// Reports where the window is, and returns without waiting.
  ///
  /// The rectangle is always the one the window has when it is *not* full-screen: macOS reports
  /// a full-screen window's frame as the whole display, and writing that down would leave
  /// somebody who leaves full-screen with a window the size of their monitor and no way back to
  /// the size they had.
  public func send(rect: NSRect?, fullScreen: Bool) {
    sender.send(Core.setWindowFrame(rect: rect, fullScreen: fullScreen))
  }
}
