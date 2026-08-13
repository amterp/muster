import Foundation
import Testing

@testable import MusterPerf

// This is the code that decides whether a build fails, so its own failure modes are the
// expensive ones: a regression that reads as clean, or a benchmark that stopped running
// and looks like one that passed.

private func cost(_ name: String, _ best: Double, unit: String = "ns/byte") -> Cost {
  Cost(name: name, unit: unit, best: best, median: best * 1.1, iterations: 50)
}

private func baseline(_ costs: [Cost]) -> Baseline {
  Baseline(recorded: "2026-08-13T00:00:00Z", machine: "test", costs: costs)
}

@Test("a cost that doubled past the tolerance fails")
func regressionIsCaught() {
  let comparison = compare(
    measured: [cost("frame.decode", 4.2)],
    against: baseline([cost("frame.decode", 2.0)]), tolerance: 2.0)

  #expect(!comparison.isClean)
  #expect(comparison.regressions.map(\.name) == ["frame.decode"])
  #expect(comparison.regressions[0].ratio == 2.1)
}

@Test("a cost inside the tolerance passes, including exactly at it")
func driftInsideToleranceIsNotAFailure() {
  // The boundary matters: a gate that fires at exactly the tolerance is a gate whose
  // documented threshold is a lie, and this one is loose precisely so it never cries wolf.
  let comparison = compare(
    measured: [cost("frame.decode", 4.0), cost("input.encode", 1.0)],
    against: baseline([cost("frame.decode", 2.0), cost("input.encode", 1.9)]),
    tolerance: 2.0)

  #expect(comparison.isClean)
  #expect(comparison.regressions.isEmpty)
}

@Test("a benchmark that stopped running is a failure, not a silent pass")
func aVanishedBenchmarkFails() {
  // cmux shipped CI that passed with every test skipped. A benchmark dropped from the run
  // produces no number to exceed anything, so absence has to be the loud case.
  let comparison = compare(
    measured: [cost("frame.decode", 2.0)],
    against: baseline([cost("frame.decode", 2.0), cost("frame.vt_parse", 9.0)]),
    tolerance: 2.0)

  #expect(!comparison.isClean)
  #expect(comparison.missing == ["frame.vt_parse"])
  #expect(comparison.regressions.isEmpty)
}

@Test("a new benchmark is reported but does not fail the run")
func anUnbaselinedBenchmarkPasses() {
  // Adding coverage must not require re-recording first, or nobody adds coverage.
  let comparison = compare(
    measured: [cost("frame.decode", 2.0), cost("mirror.apply", 700, unit: "ns/event")],
    against: baseline([cost("frame.decode", 2.0)]), tolerance: 2.0)

  #expect(comparison.isClean)
  #expect(comparison.unbaselined == ["mirror.apply"])
}

@Test("a regression says what regressed, by how much, and against what")
func theVerdictIsActionable() {
  let comparison = compare(
    measured: [cost("frame.vt_parse", 40)],
    against: baseline([cost("frame.vt_parse", 10)]), tolerance: 2.0)

  let verdict = Report.verdict(comparison, tolerance: 2.0)

  #expect(verdict.contains("frame.vt_parse"))
  #expect(verdict.contains("ns/byte"))
  #expect(verdict.contains("4.00x"))
  #expect(!verdict.contains("within budget"))
}

@Test("a clean run says so rather than saying nothing")
func aCleanVerdictIsLegible() {
  let comparison = compare(
    measured: [cost("frame.decode", 2.0)],
    against: baseline([cost("frame.decode", 2.0)]), tolerance: 2.0)

  #expect(Report.verdict(comparison, tolerance: 2.0) == "within budget")
}

@Test("the table carries every cost and its unit")
func theTableDropsNothing() {
  let table = Report.table([
    cost("frame.decode", 1.5), cost("input.encode", 820, unit: "ns/key"),
  ])

  #expect(table.contains("frame.decode"))
  #expect(table.contains("input.encode"))
  #expect(table.contains("ns/key"))
  #expect(table.split(separator: "\n").count == 3)
}

@Test("a baseline survives a round trip through its file format")
func baselineRoundTrips() throws {
  let original = baseline([cost("frame.decode", 1.25), cost("input.encode", 820)])
  let data = try JSONEncoder().encode(original)

  #expect(try JSONDecoder().decode(Baseline.self, from: data) == original)
}
