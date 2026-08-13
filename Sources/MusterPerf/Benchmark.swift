import Foundation
import MusterCore

/// Runs a body repeatedly and reports what one unit of its work costs.
public enum Benchmark {
  /// - Parameters:
  ///   - unitsPerIteration: how many bytes, keys or events one call to `body` processes.
  ///     The result is divided by this, so a benchmark that changes how much work it does
  ///     per call stays comparable to its own baseline.
  ///   - warmup: iterations whose timings are thrown away. The first pass through any of
  ///     this pays for lazy globals, a cold instruction cache, and in libghostty's case a
  ///     dylib not yet bound - none of which a running Muster pays per byte.
  public static func measure(
    name: String, unit: String, unitsPerIteration: Int, iterations: Int = 50, warmup: Int = 5,
    _ body: () -> Void
  ) -> Cost {
    precondition(unitsPerIteration > 0, "\(name): a per-unit cost needs units")

    for _ in 0..<warmup { body() }

    var samples: [Double] = []
    samples.reserveCapacity(iterations)
    for _ in 0..<iterations {
      let start = MonotonicClock.now()
      body()
      samples.append(Double(MonotonicClock.elapsed(since: start)) / Double(unitsPerIteration))
    }

    samples.sort()
    return Cost(
      name: name, unit: unit, best: samples[0], median: samples[samples.count / 2],
      iterations: iterations)
  }
}
