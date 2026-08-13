import Foundation

/// What the harness prints.
///
/// Rendering lives here rather than in the executable for the usual reason: a report is
/// read exactly when someone is deciding whether a change is safe to ship, and one that
/// silently drops a row or mislabels a unit misinforms at the worst moment.
public enum Report {
  public static func table(_ costs: [Cost]) -> String {
    guard !costs.isEmpty else { return "no costs" }

    let nameWidth = max(9, costs.map(\.name.count).max() ?? 0)
    let unitWidth = max(4, costs.map(\.unit.count).max() ?? 0)
    var lines = [
      pad("benchmark", nameWidth) + "  " + pad("unit", unitWidth) + "  "
        + lead("best") + "  " + lead("median")
    ]
    for cost in costs.sorted(by: { $0.name < $1.name }) {
      lines.append(
        pad(cost.name, nameWidth) + "  " + pad(cost.unit, unitWidth) + "  "
          + lead(number(cost.best)) + "  " + lead(number(cost.median)))
    }
    return lines.joined(separator: "\n")
  }

  /// The verdict, written so that a failure says what to do about it.
  public static func verdict(_ comparison: Comparison, tolerance: Double) -> String {
    var lines: [String] = []

    for regression in comparison.regressions.sorted(by: { $0.ratio > $1.ratio }) {
      lines.append(
        "REGRESSED \(regression.name): \(number(regression.measured)) \(regression.unit), "
          + "against a baseline of \(number(regression.baseline)) "
          + "(\(number(regression.ratio))x, tolerance \(number(tolerance))x)")
    }
    for name in comparison.missing {
      lines.append(
        "MISSING \(name): in the baseline but not measured. Either the benchmark was "
          + "removed - re-record the baseline - or it stopped running, which reads as a "
          + "pass and is not one.")
    }
    for name in comparison.unbaselined {
      lines.append("new \(name): measured but not in the baseline, so nothing gates it yet.")
    }

    if comparison.isClean && lines.isEmpty { return "within budget" }
    if comparison.isClean { return lines.joined(separator: "\n") + "\nwithin budget" }
    return lines.joined(separator: "\n")
  }

  static func number(_ value: Double) -> String {
    if value >= 100 { return String(format: "%.0f", value) }
    if value >= 10 { return String(format: "%.1f", value) }
    return String(format: "%.2f", value)
  }

  private static func pad(_ value: String, _ width: Int) -> String {
    value + String(repeating: " ", count: max(0, width - value.count))
  }

  private static func lead(_ value: String) -> String {
    String(repeating: " ", count: max(0, 9 - value.count)) + value
  }
}
