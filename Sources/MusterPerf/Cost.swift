import Foundation

/// One cost, measured.
///
/// Costs are always per unit of work rather than per run - nanoseconds per byte, per key,
/// per event - because the desideratum budgets by frequency times cardinality, and a
/// per-run number cannot be multiplied by anything.
public struct Cost: Codable, Equatable, Sendable {
  public let name: String
  /// What one unit is: `ns/byte`, `ns/key`, `ns/event`. Part of the record because a
  /// baseline compared against a differently-scaled cost is worse than no baseline.
  public let unit: String
  /// The gating number: the fastest per-unit cost observed.
  ///
  /// The minimum rather than the mean, because noise here is one-sided. A preempted
  /// sample is slower than the truth; nothing makes one faster. So the minimum is both the
  /// closest estimate of the real cost and the most reproducible number to compare across
  /// machines and runs.
  public let best: Double
  /// The middle sample, kept for information. A median far above `best` means the machine
  /// was busy, which is worth seeing before trusting the run.
  public let median: Double
  public let iterations: Int

  public init(name: String, unit: String, best: Double, median: Double, iterations: Int) {
    self.name = name
    self.unit = unit
    self.best = best
    self.median = median
    self.iterations = iterations
  }
}

/// The costs a previous run recorded, and the thing a new run is judged against.
public struct Baseline: Codable, Equatable, Sendable {
  public let recorded: String
  /// What it was measured on. A baseline from another machine explains a failure that
  /// would otherwise look like a regression.
  public let machine: String
  public let costs: [Cost]

  public init(recorded: String, machine: String, costs: [Cost]) {
    self.recorded = recorded
    self.machine = machine
    self.costs = costs
  }

  public func cost(named name: String) -> Cost? {
    costs.first { $0.name == name }
  }
}

/// A cost that grew past what the baseline allows.
public struct Regression: Equatable, Sendable {
  public let name: String
  public let unit: String
  public let baseline: Double
  public let measured: Double

  public var ratio: Double { baseline > 0 ? measured / baseline : .infinity }
}

/// How a run compares to its baseline.
public struct Comparison: Equatable, Sendable {
  public let regressions: [Regression]
  /// Measured this run but absent from the baseline. Not a failure - a new benchmark is
  /// how coverage grows - but it is un-gated until someone records it.
  public let unbaselined: [String]
  /// In the baseline but not measured this run. Worth naming: a benchmark that quietly
  /// stopped running looks exactly like one that is passing.
  public let missing: [String]

  public var isClean: Bool { regressions.isEmpty && missing.isEmpty }
}

/// Judges a run against a baseline.
///
/// - Parameter tolerance: how many times the recorded cost is still acceptable. Loose on
///   purpose. A timing gate tight enough to catch a 5% drift fires on a busy laptop
///   instead, and a gate that cries wolf gets ignored - which leaves less protection than
///   having no gate at all. This catches the regressions that matter: an accidental
///   quadratic, a copy per byte, a per-keystroke allocation storm.
public func compare(
  measured: [Cost], against baseline: Baseline, tolerance: Double = 2.0
) -> Comparison {
  var regressions: [Regression] = []
  var unbaselined: [String] = []

  for cost in measured {
    guard let recorded = baseline.cost(named: cost.name) else {
      unbaselined.append(cost.name)
      continue
    }
    if cost.best > recorded.best * tolerance {
      regressions.append(
        Regression(
          name: cost.name, unit: cost.unit, baseline: recorded.best,
          measured: cost.best))
    }
  }

  let names = Set(measured.map(\.name))
  let missing = baseline.costs.map(\.name).filter { !names.contains($0) }

  return Comparison(regressions: regressions, unbaselined: unbaselined, missing: missing)
}
