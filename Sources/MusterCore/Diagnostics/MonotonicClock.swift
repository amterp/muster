import Foundation

/// Nanoseconds on a clock that only moves forward, shared by every process on the machine.
///
/// The log spans the app and one bridge per pane, and the questions worth timing cross
/// that boundary: a keystroke leaves the app, a frame carrying its echo arrives at a
/// bridge. Wall-clock timestamps cannot answer those. They resolve milliseconds, the hops
/// are tenths of one, and they are free to jump backwards when the system adjusts time.
///
/// `CLOCK_MONOTONIC` is counted from boot rather than per process, so two records written
/// by two processes subtract directly - which is the whole reason this is worth carrying
/// on every line. POSIX, so it holds wherever the core is asked to run.
public enum MonotonicClock {
  public static func now() -> UInt64 {
    var t = timespec()
    guard clock_gettime(CLOCK_MONOTONIC, &t) == 0 else { return 0 }
    return UInt64(t.tv_sec) &* 1_000_000_000 &+ UInt64(t.tv_nsec)
  }

  /// Nanoseconds from `start` until now, or 0 if the reading failed.
  public static func elapsed(since start: UInt64) -> UInt64 {
    let end = now()
    return end > start ? end - start : 0
  }
}
