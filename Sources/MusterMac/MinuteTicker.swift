import Foundation

/// One timer that ticks on the minute, for as long as something in a window shows how long a state
/// has held.
///
/// The window owns it rather than any one view: a row, a pane's header and a card in the Needs you
/// strip all show a time, and a folded tab takes its rows off screen while its panes' headers stay.
/// A timer in the list would stop with its rows and leave the headers saying `Blocked 1m` an hour
/// later. One for the window is one wakeup a minute at fifteen panes, where one per view was
/// fifteen.
@MainActor
final class MinuteTicker {
  /// What the time is, replaceable so that a test can let a minute pass without waiting for one.
  var clock: () -> Date = { Date() }

  private let isNeeded: () -> Bool
  private let onTick: () -> Void
  private var timer: Timer?

  /// Whether the timer is running, which it is only while something shows a time.
  var isTicking: Bool { timer != nil }

  /// `needed` says whether anything shows a time; `tick` redraws what does.
  init(needed: @escaping () -> Bool, tick: @escaping () -> Void) {
    isNeeded = needed
    onTick = tick
  }

  /// Starts the timer when something shows a time and nothing is counting, and ends it when
  /// nothing does. Called after anything that can change which of the two is true.
  func reconsider() {
    let needed = isNeeded()
    guard needed != isTicking else { return }
    guard needed else {
      timer?.invalidate()
      timer = nil
      return
    }
    let now = clock().timeIntervalSince1970
    let next = Date(timeIntervalSince1970: (now / 60).rounded(.down) * 60 + 60)
    // A ticker that has gone - its window closed - ends its own timer: the run loop holds a
    // repeating timer for as long as it is valid, and nothing else would stop this one.
    let timer = Timer(fire: next, interval: 60, repeats: true) { [weak self] timer in
      guard let self else {
        timer.invalidate()
        return
      }
      MainActor.assumeIsolated { self.fire() }
    }
    timer.tolerance = 1
    RunLoop.main.add(timer, forMode: .common)
    self.timer = timer
  }

  /// One minute passing: what the timer calls, and what a test calls after moving `clock`.
  func fire() {
    onTick()
    reconsider()
  }

  /// Ends the timer, for a window that is closing.
  func stop() {
    timer?.invalidate()
    timer = nil
  }
}
