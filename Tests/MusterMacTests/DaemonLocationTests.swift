import Foundation
import Testing

@testable import MusterMac

// Finding the wrong daemon does not fail loudly. It attaches a daemon this build was never
// tested against, and every behaviour after that is unverified.

@Test("a build's daemon is found next to the app, not on PATH")
func daemonSitsBesideTheExecutable() throws {
  // `./dev` stages the daemon and its data directory beside the app for a SwiftPM build.
  // Resolving by name would find whatever a developer happens to have installed.
  let directory = try scratchDirectory()
  let staged = try executable(directory.appendingPathComponent("muster-daemon"))

  let found = daemonLocation(
    executable: directory.appendingPathComponent("muster").path, environment: [:])

  #expect(found == DaemonLocation(binary: staged, data: nil))
}

@Test("a bundle's daemon is the one inside its helper, with the data among its resources")
func theBundleCarriesTheDaemon() throws {
  // A code directory may hold only code, so the data directory cannot sit beside the binary in
  // a signed bundle. Nil here would send the daemon looking beside itself and finding nothing,
  // and every shell it started would come up without terminfo or shell integration.
  let contents = try scratchDirectory().appendingPathComponent("Contents", isDirectory: true)
  let executables = contents.appendingPathComponent("MacOS", isDirectory: true)
  let helper = contents.appendingPathComponent("Library/MusterSessions.app/Contents")
  try FileManager.default.createDirectory(at: executables, withIntermediateDirectories: true)
  let bundled = try executable(helper.appendingPathComponent("MacOS/muster-daemon"))

  let found = daemonLocation(
    executable: executables.appendingPathComponent("muster").path, environment: [:])

  #expect(
    found
      == DaemonLocation(
        binary: bundled,
        data: helper.appendingPathComponent("Resources/muster-daemon-data").path))
}

@Test("the helper bundle wins over a daemon beside the app")
func theBundleIsPreferred() throws {
  // A bundle is what ships, and its data directory is where only the bundle case looks. A
  // stray binary beside the app winning would start a daemon that cannot find its data.
  let contents = try scratchDirectory().appendingPathComponent("Contents", isDirectory: true)
  let executables = contents.appendingPathComponent("MacOS", isDirectory: true)
  let helper = contents.appendingPathComponent("Library/MusterSessions.app/Contents")
  _ = try executable(executables.appendingPathComponent("muster-daemon"))
  let bundled = try executable(helper.appendingPathComponent("MacOS/muster-daemon"))

  let found = daemonLocation(
    executable: executables.appendingPathComponent("muster").path, environment: [:])

  #expect(found?.binary == bundled)
}

@Test("MUSTER_DAEMON_BINARY wins, and names only a binary")
func theOverrideWins() throws {
  // For running a daemon somebody just built. Its data is wherever that build put it, which is
  // beside it - so nothing is said about data, and the daemon looks there.
  let contents = try scratchDirectory().appendingPathComponent("Contents", isDirectory: true)
  let executables = contents.appendingPathComponent("MacOS", isDirectory: true)
  _ = try executable(executables.appendingPathComponent("muster-daemon"))
  _ = try executable(
    contents.appendingPathComponent("Library/MusterSessions.app/Contents/MacOS/muster-daemon"))

  let found = daemonLocation(
    executable: executables.appendingPathComponent("muster").path,
    environment: ["MUSTER_DAEMON_BINARY": "/elsewhere/muster-daemon"])

  #expect(found == DaemonLocation(binary: "/elsewhere/muster-daemon", data: nil))
}

@Test("MUSTER_DAEMON is not an override")
func thePanesDaemonIsNotAnOverride() throws {
  // Every pane's environment names the daemon it runs on this way, so a Muster launched from a
  // pane of another install would otherwise take that install's daemon for its own.
  let directory = try scratchDirectory()
  let staged = try executable(directory.appendingPathComponent("muster-daemon"))

  let found = daemonLocation(
    executable: directory.appendingPathComponent("muster").path,
    environment: ["MUSTER_DAEMON": "/Applications/Other.app/muster-daemon"])

  #expect(found?.binary == staged)
}

@Test("a build that staged no daemon says so rather than guessing")
func noDaemonIsNil() throws {
  // Nil reaches the core as "none staged", which reports a window with nothing behind it.
  // Falling back to PATH here is what would make that failure silent.
  let directory = try scratchDirectory()

  #expect(
    daemonLocation(executable: directory.appendingPathComponent("muster").path, environment: [:])
      == nil)
}

@Test("a file that is not executable is not a daemon")
func aNonExecutableIsIgnored() throws {
  // A half-finished copy beside the app would otherwise be handed over as the daemon to start,
  // and the failure would arrive as a spawn error at launch.
  let directory = try scratchDirectory()
  FileManager.default.createFile(
    atPath: directory.appendingPathComponent("muster-daemon").path, contents: Data(),
    attributes: [.posixPermissions: 0o644])

  #expect(
    daemonLocation(executable: directory.appendingPathComponent("muster").path, environment: [:])
      == nil)
}

private func executable(_ file: URL) throws -> String {
  try FileManager.default.createDirectory(
    at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
  FileManager.default.createFile(
    atPath: file.path, contents: Data(), attributes: [.posixPermissions: 0o755])
  return file.path
}

private func scratchDirectory() throws -> URL {
  let directory = FileManager.default.temporaryDirectory
    .appendingPathComponent("muster-daemon-location-\(UUID().uuidString)", isDirectory: true)
  try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
  return directory
}
