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

@Test("the helper bundle wins over the binary beside the app")
func theBundleIsPreferred() throws {
  // What macOS charges a pane's protected request to is decided by how the daemon was
  // started, and only a bundle can be started through Launch Services. A build that carries
  // both must hand over the bundle, or every pane goes back to being charged to Muster until
  // Muster exits and to nothing nameable after that.
  let contents = try scratchDirectory().appendingPathComponent("Contents", isDirectory: true)
  let executables = contents.appendingPathComponent("MacOS", isDirectory: true)
  let bundle =
    contents
    .appendingPathComponent("Library", isDirectory: true)
    .appendingPathComponent("MusterSessions.app", isDirectory: true)
  try FileManager.default.createDirectory(at: executables, withIntermediateDirectories: true)
  try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
  FileManager.default.createFile(
    atPath: executables.appendingPathComponent("herdr").path, contents: Data(),
    attributes: [.posixPermissions: 0o755])

  let found = herdrPath(
    executable: executables.appendingPathComponent("muster").path, environment: [:])

  #expect(found == bundle.path)
}

@Test("a plain build with no bundle keeps the binary beside it")
func withoutABundleTheBinaryStands() throws {
  // `./dev` stages a bare binary for a plain `swift build`, and every test in this repo uses
  // one. Both keep the spawn they always had, so this is the answer that must not move.
  let contents = try scratchDirectory().appendingPathComponent("Contents", isDirectory: true)
  let executables = contents.appendingPathComponent("MacOS", isDirectory: true)
  try FileManager.default.createDirectory(at: executables, withIntermediateDirectories: true)
  let staged = executables.appendingPathComponent("herdr").path
  FileManager.default.createFile(
    atPath: staged, contents: Data(), attributes: [.posixPermissions: 0o755])

  let found = herdrPath(
    executable: executables.appendingPathComponent("muster").path, environment: [:])

  #expect(found == staged)
}

@Test("MUSTER_HERDR wins over the bundle too")
func theOverrideBeatsTheBundle() throws {
  // The override exists to bisect herdr, which means pointing it at a build somebody just
  // made rather than at a bundle. A bundle silently outranking it would make the override
  // look like it did nothing.
  let contents = try scratchDirectory().appendingPathComponent("Contents", isDirectory: true)
  let executables = contents.appendingPathComponent("MacOS", isDirectory: true)
  let bundle =
    contents
    .appendingPathComponent("Library", isDirectory: true)
    .appendingPathComponent("MusterSessions.app", isDirectory: true)
  try FileManager.default.createDirectory(at: executables, withIntermediateDirectories: true)
  try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)

  let found = herdrPath(
    executable: executables.appendingPathComponent("muster").path,
    environment: ["MUSTER_HERDR": "/elsewhere/herdr"])

  #expect(found == "/elsewhere/herdr")
}

// The bridge cannot exec a bundle, so the two callers need different answers to one question.
// Getting that wrong is what shipped: kan a_2Hnh3g0Y5, where every pane of the released cask
// rendered nothing because a rule the app had updated was duplicated somewhere that had not.

@Test("a bridge is given the binary inside the helper bundle, not the bundle")
func theBridgeGetsAnExecutable() throws {
  // `Command::new` on a directory is ENOENT at best. The core wants the bundle, because only a
  // process Launch Services started is its own for what macOS charges a pane's permission
  // prompts to; a bridge wants the Mach-O inside it.
  let contents = try scratchDirectory().appendingPathComponent("Contents", isDirectory: true)
  let executables = contents.appendingPathComponent("MacOS", isDirectory: true)
  let bundle =
    contents
    .appendingPathComponent("Library", isDirectory: true)
    .appendingPathComponent("MusterSessions.app", isDirectory: true)
  try FileManager.default.createDirectory(at: executables, withIntermediateDirectories: true)
  try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)

  let found = herdrBinaryPath(
    executable: executables.appendingPathComponent("muster").path, environment: [:])

  #expect(found == bundle.appendingPathComponent("Contents/MacOS/herdr").path)
}

@Test("a plain build hands over the binary it staged")
func aPlainBuildIsAlreadyAnExecutable() throws {
  // `./dev` stages the pinned daemon beside the binaries, which is the layout every test in
  // this repo runs against. It must arrive unchanged.
  let directory = try scratchDirectory()
  let staged = directory.appendingPathComponent("herdr").path
  FileManager.default.createFile(
    atPath: staged, contents: Data(), attributes: [.posixPermissions: 0o755])

  let found = herdrBinaryPath(
    executable: directory.appendingPathComponent("muster").path, environment: [:])

  #expect(found == staged)
}

@Test("MUSTER_HERDR reaches the bridge as it was written")
func theOverrideReachesTheBridge() throws {
  // The override names a binary, which is the point of it - somebody bisecting herdr has just
  // built one. Rewriting it into a bundle's innards would name nothing.
  let directory = try scratchDirectory()

  let found = herdrBinaryPath(
    executable: directory.appendingPathComponent("muster").path,
    environment: ["MUSTER_HERDR": "/elsewhere/herdr"])

  #expect(found == "/elsewhere/herdr")
}

@Test("a build that staged no daemon tells the bridge nothing")
func noDaemonMeansNoArgument() throws {
  // Nil becomes an absent argument rather than an empty one, and the bridge then falls back to
  // looking - which is right for a bridge somebody ran by hand.
  let directory = try scratchDirectory()

  #expect(
    herdrBinaryPath(
      executable: directory.appendingPathComponent("muster").path, environment: [:]) == nil)
}

private func scratchDirectory() throws -> URL {
  let directory = FileManager.default.temporaryDirectory
    .appendingPathComponent("muster-herdr-location-\(UUID().uuidString)", isDirectory: true)
  try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
  return directory
}
