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
final class ViewSequence: Sendable {
  private let issued = OSAllocatedUnfairLock(initialState: UInt64(0))

  /// Numbers a view that has just arrived.
  func next() -> UInt64 {
    issued.withLock {
      $0 += 1
      return $0
    }
  }

  /// Whether no view has arrived since this one.
  func isLatest(_ view: UInt64) -> Bool { issued.withLock { $0 == view } }
}
