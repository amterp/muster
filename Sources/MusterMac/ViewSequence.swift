import os

/// Which view the core published last, so that one with a newer view behind it is not applied.
///
/// Every event the core sends becomes its own task on the main actor, in order. A view is the
/// whole window rather than a change to it, so applying one that a newer one has replaced does
/// nothing but cost the main thread - except where it differs from the window it replaced in a
/// pane's `bridge_restarts`, which is a surface torn down and a bridge started for a number the
/// core has already moved past. A window whose main thread had fallen minutes behind did that
/// once for every number, each new bridge taking the pane from the last, and freed a surface at
/// the moment that hung it (docs/observations/libghostty-9f9b8d1d.md, section 15).
///
/// Numbered as they arrive, on the core's thread, and asked on the main actor, so a lock rather
/// than either one's isolation.
///
/// Counted per window. A window's view says nothing about another's, so a view for one window
/// arriving behind a view for another replaces nothing.
final class ViewSequence: Sendable {
  private let issued = OSAllocatedUnfairLock(initialState: [String: UInt64]())

  /// Numbers a view for `window` that has just arrived.
  func next(for window: String) -> UInt64 {
    issued.withLock {
      let next = $0[window, default: 0] + 1
      $0[window] = next
      return next
    }
  }

  /// Whether no view for the same window has arrived since this one.
  func isLatest(_ view: UInt64, for window: String) -> Bool {
    issued.withLock { $0[window] == view }
  }
}
