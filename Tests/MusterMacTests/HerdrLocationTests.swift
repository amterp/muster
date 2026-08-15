import Foundation
import Testing

@testable import MusterMac

// Finding the wrong herdr does not fail loudly. It attaches a daemon of a version this
// project's corpus says nothing about, and every behaviour after that is unverified.

@Test("the daemon is found next to the app, not on PATH")
func daemonSitsBesideTheExecutable() throws {
  // Both come out of the same place: `./dev` stages the pinned binary beside the app, and a
  // bundle carries it in Contents/MacOS. Resolving by name would find whatever a developer
  // happens to have installed.
  let directory = try scratchDirectory()
  let staged = directory.appendingPathComponent("herdr").path
  FileManager.default.createFile(
    atPath: staged, contents: Data(), attributes: [.posixPermissions: 0o755])

  let found = herdrPath(
    executable: directory.appendingPathComponent("muster").path, environment: [:])

  #expect(found == staged)
}

@Test("a build that staged no daemon says so rather than guessing")
func noDaemonIsNil() throws {
  // Nil reaches the core as "none staged", which reports a window with nothing behind it.
  // Falling back to PATH here is what would make that failure silent.
  let directory = try scratchDirectory()

  #expect(
    herdrPath(executable: directory.appendingPathComponent("muster").path, environment: [:])
      == nil)
}

@Test("a file that is not executable is not a daemon")
func aNonExecutableIsIgnored() throws {
  // A half-finished copy or a stray text file beside the app would otherwise be handed over
  // as the daemon to start, and the failure would arrive as a spawn error at launch.
  let directory = try scratchDirectory()
  let staged = directory.appendingPathComponent("herdr").path
  FileManager.default.createFile(
    atPath: staged, contents: Data(), attributes: [.posixPermissions: 0o644])

  #expect(
    herdrPath(executable: directory.appendingPathComponent("muster").path, environment: [:])
      == nil)
}

@Test("MUSTER_HERDR wins, so herdr itself can be bisected")
func theOverrideWins() throws {
  // The suite already spells the override this way. A second name for the same thing is a
  // second thing to remember, and the wrong one fails by doing nothing.
  let directory = try scratchDirectory()
  let staged = directory.appendingPathComponent("herdr").path
  FileManager.default.createFile(
    atPath: staged, contents: Data(), attributes: [.posixPermissions: 0o755])

  let found = herdrPath(
    executable: directory.appendingPathComponent("muster").path,
    environment: ["MUSTER_HERDR": "/elsewhere/herdr"])

  #expect(found == "/elsewhere/herdr")
}

private func scratchDirectory() throws -> URL {
  let directory = FileManager.default.temporaryDirectory
    .appendingPathComponent("muster-herdr-location-\(UUID().uuidString)", isDirectory: true)
  try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
  return directory
}
