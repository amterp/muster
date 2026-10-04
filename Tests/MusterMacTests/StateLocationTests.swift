import Foundation
import Testing

@testable import MusterMac

// Which arrangement a window opens onto. Every wrong answer here is a window that forgets its
// layout, or two windows writing over each other's, or one that cannot be brought back after it
// closes.

/// A `MUSTER_HOME` of its own per test, emptied first.
///
/// A real directory rather than an injected filesystem: what this answers is which files are there
/// and when each was written, and a stand-in for that would be a stand-in for the whole question.
/// Under this process's pid, so another checkout running these at once does not empty it.
private func scratch(_ named: String) -> String {
  let home = "/tmp/muster-state-tests/\(getpid())-\(named)"
  try? FileManager.default.removeItem(atPath: home)
  try? FileManager.default.createDirectory(
    atPath: home, withIntermediateDirectories: true)
  return home
}

/// The install's own state directory under a scratch home, as a claim on the app answers it.
private func state(_ home: String) -> URL {
  URL(fileURLWithPath: home).appendingPathComponent("state/i", isDirectory: true)
}

/// The records the directory holds, by name.
private func slots(_ home: String) -> [String] {
  let directory = state(home).appendingPathComponent("windows")
  return Arrangements.slots(in: directory).map { $0.stem }.sorted()
}

/// Writes something into a record, which is what the core does once the window settles.
private func publish(_ path: String?) {
  try? "version = 3\n".write(toFile: path!, atomically: true, encoding: .utf8)
}

/// Opens an arrangement in a scratch home, with `open` the ones the app's windows have open.
private func open(
  _ home: String, fresh: Bool = false, named: String? = nil, open: Set<String> = []
) -> String? {
  Arrangements.open(
    fresh: fresh, named: named, environment: ["MUSTER_HOME": home], state: state(home),
    open: open)
}

@Test func aFirstLaunchTakesARecordOfItsOwn() {
  let home = scratch("first")
  let taken = open(home)

  #expect(taken == "\(home)/state/i/windows/window-1.toml")
  // Nothing on disk yet: the core writes the record once the window settles.
  #expect(slots(home).isEmpty)
}

@Test func aWindowSomebodyAskedForTakesADifferentRecord() {
  // The bug this whole arrangement is about: while there was one file, two windows read and
  // wrote it in turn and whichever published last decided what came back.
  let home = scratch("asked-for")
  let first = open(home)!
  let second = open(home, fresh: true, open: [first])

  #expect(first != second)
  #expect(second == "\(home)/state/i/windows/window-2.toml")
}

@Test func aWindowDoesNotTakeARecordAnotherWindowHasOpen() {
  // Two windows writing one record is the two-windows bug again, even before the first has
  // written anything into it.
  let home = scratch("held")
  let held = open(home)!
  publish(held)

  #expect(open(home, open: [held]) != held)
}

@Test func theRecordOfAWindowThatClosedComesBack() {
  // A record no window has open, with something written in it, is exactly what "the window I
  // just closed" means.
  let home = scratch("reopened")
  let closed = open(home)
  publish(closed)

  #expect(open(home) == closed)
}

@Test func aRecordNothingWasWrittenIntoIsNotReopened() {
  // A window that opened and closed before it published left nothing to come back to, and taking
  // its record would look like a window that forgot everything.
  let home = scratch("unwritten")
  let empty = open(home)!
  let written = open(home, fresh: true, open: [empty])
  publish(written)

  #expect(open(home) == written)
}

@Test func theOneFileEveryWindowUsedToShareBecomesTheFirstRecord() {
  let home = scratch("adopted")
  try? FileManager.default.createDirectory(
    atPath: "\(home)/state", withIntermediateDirectories: true)
  try? "version = 3\n".write(
    toFile: "\(home)/state/window.toml", atomically: true, encoding: .utf8)

  let taken = open(home)

  #expect(taken?.hasSuffix("/state/i/windows/window-1.toml") == true)
  #expect((try? String(contentsOfFile: taken!, encoding: .utf8)) == "version = 3\n")
  #expect(!FileManager.default.fileExists(atPath: "\(home)/state/window.toml"))
}

@Test func anExplicitStatePathWins() {
  // What a test and a script want: one named file, and no directory of records beside it.
  let home = scratch("explicit")
  let taken = Arrangements.open(
    fresh: false, environment: ["MUSTER_STATE": "/tmp/one.toml", "MUSTER_HOME": home],
    state: state(home))

  #expect(taken == "/tmp/one.toml")
  #expect(slots(home).isEmpty)
}

@Test func anEmptyExplicitStatePathMeansRememberNothing() {
  // The difference between "look somewhere else" and "do not remember", which a script or a
  // test needs and which an absent variable cannot say.
  let home = scratch("nothing")
  #expect(
    Arrangements.open(
      fresh: false, environment: ["MUSTER_STATE": "", "MUSTER_HOME": home], state: state(home))
      == nil)
}

@Test func nowhereToWriteIsAnAnswer() {
  // A window that opens fresh every time, which is what it did before any of this existed -
  // rather than a path built from an empty base, which would name something in the
  // filesystem root.
  #expect(Arrangements.open(fresh: false, environment: [:], state: nil) == nil)
}

@Test func recordsSitInTheInstallsOwnStateDirectory() {
  // Not beside the config file, which a person writes, and not beside another install's: a
  // development build and the release each keep their own windows (mip/0006-one-process.md,
  // section 5).
  let release = URL(fileURLWithPath: "/home/a/.muster/state/release", isDirectory: true)
  #expect(Arrangements.directory(state: release)?.path == "/home/a/.muster/state/release/windows")
  #expect(Arrangements.directory(state: nil) == nil)
  #expect(
    tabHoldersPath(environment: [:], state: release)
      == "/home/a/.muster/state/release/holding/tabs.toml")
}

@Test func aClosedWindowIsReopenedFromItsOwnRecord() {
  // Going to a closed window's tab reopens that window, and a window is its record (kan
  // a_2Mhi0EZlv) - so it takes the record it was told, not the newest free one.
  let home = scratch("named")
  let first = open(home)!
  publish(first)
  let second = open(home, fresh: true, open: [first])!
  publish(second)

  #expect(open(home, named: "window-1") == first)
}

@Test func aNameAWindowHereHasOpenIsNotReopenedAgain() {
  // That window is open already, and taking its record too would be two windows writing one
  // file. The shell then opens nothing (`WindowOpening`), since it was given another record.
  let home = scratch("named-held")
  let held = open(home)!
  publish(held)

  #expect(open(home, named: "window-1", open: [held]) != held)
}

@Test func aNameLeftInTheSharedDirectoryIsNotGivenToANewWindow() {
  // A window process from before one app per install that would not quit keeps its arrangement
  // in the directory every install shared, until a later launch adopts it under its name. A new
  // window named the same would take its row in the record, and its tabs with it.
  let home = scratch("left-behind")
  let shared = URL(fileURLWithPath: home).appendingPathComponent("state/windows")
  try? FileManager.default.createDirectory(at: shared, withIntermediateDirectories: true)
  publish(shared.appendingPathComponent("window-1.toml").path)

  #expect(open(home, fresh: true) == "\(home)/state/i/windows/window-2.toml")
}
