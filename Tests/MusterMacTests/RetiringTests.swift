import Foundation
import Testing

@testable import MusterMac

// Which window processes from before one app per install are asked to quit. A wrong answer either
// leaves an old window fighting this app for its tabs, or quits a process that was none of its
// business - another checkout's build, or this app itself.

private func scratch(_ named: String) -> URL {
  let directory = URL(fileURLWithPath: "/tmp/muster-retiring-tests/\(named)", isDirectory: true)
  try? FileManager.default.removeItem(at: directory)
  try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
  return directory
}

@Test func onlyAnotherProcessOfThisBundleIsAskedToQuit() {
  let app = URL(fileURLWithPath: "/Applications/Muster.app", isDirectory: true)
  let other = URL(fileURLWithPath: "/src/muster-2/.build/muster.app", isDirectory: true)
  let holders = [
    Retiring.Holder(pid: 10, stem: "window-1"),
    Retiring.Holder(pid: 11, stem: "window-2"),
    Retiring.Holder(pid: 12, stem: "window-3"),
    Retiring.Holder(pid: 13, stem: "window-4"),
  ]
  let bundles: [pid_t: URL] = [10: app, 11: other, 12: app]

  let asked = Retiring.sameBundle(holders, own: 12, bundle: app, bundleOf: { bundles[$0] })

  // 11 is another build, 12 is this process, and 13 is not an app at all.
  #expect(asked == [Retiring.Holder(pid: 10, stem: "window-1")])
}

@Test func aBuildThatIsNoBundleAsksNobody() {
  // A SwiftPM build runs no bundle, so there is no bundle for another process to share.
  let holders = [Retiring.Holder(pid: 10, stem: "window-1")]
  #expect(Retiring.sameBundle(holders, own: 1, bundle: nil, bundleOf: { _ in nil }).isEmpty)
}

@Test func aClaimNamesItsArrangementAndItsProcess() {
  let directory = scratch("claims")
  let live = ProcessInfo.processInfo.processIdentifier
  try? String(live).write(
    to: directory.appendingPathComponent("window-2.held"), atomically: true, encoding: .utf8)
  try? "version = 3\n".write(
    to: directory.appendingPathComponent("window-2.toml"), atomically: true, encoding: .utf8)
  // A pid no process has, which is a window that was killed: not a process to ask anything.
  try? "99999".write(
    to: directory.appendingPathComponent("window-3.held"), atomically: true, encoding: .utf8)

  #expect(Retiring.claims(in: directory) == [Retiring.Holder(pid: live, stem: "window-2")])
}

@Test func theWindowsWhoseProcessesQuitReopenFromWhereTheyWereAdopted() {
  let state = scratch("adopted")
  let windows = state.appendingPathComponent("windows")
  try? FileManager.default.createDirectory(at: windows, withIntermediateDirectories: true)
  try? "version = 3\n".write(
    to: windows.appendingPathComponent("window-1.toml"), atomically: true, encoding: .utf8)
  let holders = [
    Retiring.Holder(pid: 10, stem: "window-1"),
    Retiring.Holder(pid: 11, stem: "window-2"),
    Retiring.Holder(pid: 12, stem: "window-3"),
  ]

  let outcome = Retiring.outcome(of: holders, stillRunning: [12])

  #expect(outcome.quit == ["window-1", "window-2"])
  #expect(outcome.stayed == [12])
  // window-2 quit but was not adopted - its arrangement is not here to open.
  #expect(
    Retiring.arrangements(outcome, in: state) == [
      windows.appendingPathComponent("window-1.toml").path
    ])
}
