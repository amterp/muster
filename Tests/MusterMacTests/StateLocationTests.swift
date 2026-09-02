import Foundation
import Testing

@testable import MusterMac

// Which arrangement a launch adopts. Every wrong answer here is a window that forgets its layout,
// or two windows writing over each other's, or one that cannot be brought back after it closes.

/// A `MUSTER_HOME` of its own per test, removed afterwards.
///
/// A real directory rather than an injected filesystem: what this answers is which files are there
/// and which of them a live process is holding, and a stand-in for that would be a stand-in for
/// the whole question.
private func scratch(_ named: String) -> String {
  let home = "/tmp/muster-state-tests/\(named)"
  try? FileManager.default.removeItem(atPath: home)
  try? FileManager.default.createDirectory(
    atPath: home, withIntermediateDirectories: true)
  return home
}

/// The slots the directory holds, by name.
private func slots(_ home: String) -> [String] {
  let directory = URL(fileURLWithPath: home).appendingPathComponent("state/windows")
  return Arrangements.slots(in: directory).map { $0.stem }.sorted()
}

/// Writes something into a record, which is what the core does once the window settles.
private func publish(_ path: String?) {
  try? "version = 3\n".write(toFile: path!, atomically: true, encoding: .utf8)
}

@Test func aFirstLaunchTakesARecordOfItsOwn() {
  let home = scratch("first")
  let taken = Arrangements.open(fresh: false, environment: ["MUSTER_HOME": home], pid: 4001)

  #expect(taken == "\(home)/state/windows/window-1.toml")
  #expect(slots(home) == ["window-1"])
}

@Test func aWindowSomebodyAskedForTakesADifferentRecord() {
  // The bug this whole arrangement is about: while there was one file, two windows read and
  // wrote it in turn and whichever published last decided what came back.
  let home = scratch("asked-for")
  // This process's own pid for both, which is what two windows running at once looks like from
  // here: a claim naming a pid that is gone is swept before the next launch chooses.
  let live = ProcessInfo.processInfo.processIdentifier
  let first = Arrangements.open(fresh: false, environment: ["MUSTER_HOME": home], pid: live)
  let second = Arrangements.open(fresh: true, environment: ["MUSTER_HOME": home], pid: live)

  #expect(first != second)
  #expect(slots(home) == ["window-1", "window-2"])
}

@Test func aLaunchDoesNotTakeARecordALiveWindowIsHolding() {
  // Two windows Muster comes back to is what a second launch looks like when the first is still
  // running, and taking its record would put both of them back where the two-windows bug was.
  let home = scratch("held")
  let held = Arrangements.open(
    fresh: false, environment: ["MUSTER_HOME": home], pid: ProcessInfo.processInfo.processIdentifier
  )
  let next = Arrangements.open(fresh: false, environment: ["MUSTER_HOME": home], pid: 4004)

  #expect(held != next)
}

@Test func theRecordOfAWindowThatIsGoneComesBack() {
  // The gap this closes: nothing brought back a window you had closed. A record whose window is
  // no longer running is exactly what "the window I just closed" means.
  let home = scratch("reopened")
  let closed = Arrangements.open(fresh: false, environment: ["MUSTER_HOME": home], pid: 4005)
  publish(closed)
  // 4005 is not running, so the claim it left is stale and the next launch takes the record.
  let reopened = Arrangements.open(fresh: false, environment: ["MUSTER_HOME": home], pid: 4006)

  #expect(reopened == closed)
}

@Test func givingUpARecordLetsTheNextLaunchTakeIt() {
  // Quitting and reopening inside the same second, before anything has swept a dead claim.
  let home = scratch("released")
  let live = ProcessInfo.processInfo.processIdentifier
  let closed = Arrangements.open(fresh: false, environment: ["MUSTER_HOME": home], pid: live)
  publish(closed)
  Arrangements.release(closed!)
  let reopened = Arrangements.open(fresh: false, environment: ["MUSTER_HOME": home], pid: live)

  #expect(reopened == closed)
}

@Test func theOneFileEveryWindowUsedToShareBecomesTheFirstRecord() {
  // Moved rather than read, so the arrangement somebody had when they upgraded is the one their
  // window comes back to.
  let home = scratch("upgrade")
  try? FileManager.default.createDirectory(
    atPath: "\(home)/state", withIntermediateDirectories: true)
  try? "version = 3\n".write(
    toFile: "\(home)/state/window.toml", atomically: true, encoding: .utf8)

  let taken = Arrangements.open(fresh: false, environment: ["MUSTER_HOME": home], pid: 4007)

  #expect(taken?.hasSuffix("/state/windows/window-1.toml") == true)
  #expect((try? String(contentsOfFile: taken!, encoding: .utf8)) == "version = 3\n")
  #expect(!FileManager.default.fileExists(atPath: "\(home)/state/window.toml"))
}

@Test func anExplicitStatePathWinsAndClaimsNothing() {
  // What a test and a script want: one named file, and no directory of records beside it.
  let home = scratch("explicit")
  let taken = Arrangements.open(
    fresh: false, environment: ["MUSTER_STATE": "/tmp/one.toml", "MUSTER_HOME": home], pid: 4008)

  #expect(taken == "/tmp/one.toml")
  #expect(slots(home).isEmpty)
}

@Test func anEmptyExplicitStatePathMeansRememberNothing() {
  // The difference between "look somewhere else" and "do not remember", which a script or a
  // test needs and which an absent variable cannot say.
  let home = scratch("nothing")
  #expect(
    Arrangements.open(
      fresh: false, environment: ["MUSTER_STATE": "", "MUSTER_HOME": home], pid: 4009) == nil)
}

@Test func nowhereToWriteIsAnAnswer() {
  // A window that opens fresh every time, which is what it did before any of this existed -
  // rather than a path built from an empty base, which would name something in the
  // filesystem root.
  #expect(Arrangements.open(fresh: false, environment: [:], pid: 4010) == nil)
}

@Test func recordsSitWhereRegeneratedFilesBelong() {
  // Not beside the config file. That one is written by a person and would be surprising to
  // find rewritten; these are written by Muster on every change.
  #expect(
    Arrangements.directory(environment: ["HOME": "/home/a"])?.path
      == "/home/a/.muster/state/windows")
}

@Test func xdgDoesNotMoveTheRecords() {
  // It did until Muster's files moved into one home, and it still moves herdr's - so a reader
  // could reasonably expect it to move these too. Pinned rather than assumed.
  #expect(
    Arrangements.directory(environment: ["XDG_STATE_HOME": "/xdg", "HOME": "/home/a"])?.path
      == "/home/a/.muster/state/windows")
}

// Where what Muster calls each pane is remembered. One file for all of them, unlike an
// arrangement, because the names belong to panes that outlive every window.

@Test func paneNamesSitBesideTheArrangements() {
  #expect(paneNamesPath(environment: ["HOME": "/home/a"]) == "/home/a/.muster/state/panes.toml")
}

@Test func musterHomeMovesThePaneNames() {
  #expect(
    paneNamesPath(environment: ["MUSTER_HOME": "/scratch", "HOME": "/home/a"])
      == "/scratch/state/panes.toml")
}

@Test func anEmptyExplicitPaneNamesPathMeansNameAfresh() {
  // A test or a script saying "start with no names". Every pane is named again, which is only
  // safe because nothing was running in one yet.
  #expect(paneNamesPath(environment: ["MUSTER_PANE_NAMES": "", "HOME": "/home/a"]) == nil)
}
