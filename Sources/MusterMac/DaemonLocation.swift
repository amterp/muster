import Foundation

// Where the daemon Muster ships actually sits, which is the one part of owning it that is an
// OS and packaging question. The core starts it and talks to it; this decides where it lives
// relative to the running executable, and hands the path over at startup - the same division
// the log file and the config file already draw.

/// The helper bundle a real Muster.app carries its daemon in.
///
/// `Contents/Library/` is where a bundle puts a helper application, one directory up from the
/// executable that looks for it. The daemon keeps its data directory among the helper's
/// resources, because a code directory may hold only code and the data is terminfo and shell
/// scripts.
private let daemonBundle = "MusterSessions.app"

/// The daemon this build ships, and the directory of terminfo and shell integration it gives
/// every shell it starts.
public struct DaemonLocation: Equatable {
  public let binary: String

  /// Nil for the directory beside the binary, which is where a build stages it and where the
  /// daemon looks when it is told nothing.
  public let data: String?

  /// The Linux daemons a remote install sends, one directory per build, or nil for a build
  /// that carries none.
  public let remote: String?

  public init(binary: String, data: String?, remote: String? = nil) {
    self.binary = binary
    self.data = data
    self.remote = remote
  }
}

/// This build's daemon, or nil if it has none.
///
/// Inside the helper bundle for a shipped app, and beside the executable for a SwiftPM build,
/// which is what `./dev` stages and what every test uses. Deliberately not PATH: the daemon
/// Muster starts should be the version this build was tested against.
///
/// `MUSTER_DAEMON_BINARY` overrides both, for running a daemon somebody just built. Not
/// `MUSTER_DAEMON`, which every pane's environment already carries naming the daemon that
/// pane runs on - so a Muster launched from inside a pane would take that install's daemon
/// for its own.
///
/// Nil means this build staged no daemon. A real state rather than a default to paper over -
/// the core reports it as a window with nothing behind it, which is what it is.
///
/// Takes the environment rather than reading it, so the rules are answerable without one - and
/// so a developer who exports the override for their own work does not change what the suite
/// is testing.
public func daemonLocation(
  executable: String,
  environment: [String: String] = ProcessInfo.processInfo.environment
) -> DaemonLocation? {
  let macOS = URL(fileURLWithPath: executable).deletingLastPathComponent()
  let remote = carriedDaemons(macOS: macOS)
  if let explicit = environment["MUSTER_DAEMON_BINARY"], !explicit.isEmpty {
    return DaemonLocation(binary: explicit, data: nil, remote: remote)
  }

  let contents = macOS.deletingLastPathComponent()
    .appendingPathComponent("Library")
    .appendingPathComponent(daemonBundle)
    .appendingPathComponent("Contents")
  let bundled = contents.appendingPathComponent("MacOS").appendingPathComponent("muster-daemon")
  if FileManager.default.isExecutableFile(atPath: bundled.path) {
    return DaemonLocation(
      binary: bundled.path,
      data: contents.appendingPathComponent("Resources")
        .appendingPathComponent("muster-daemon-data").path,
      remote: remote)
  }

  let beside = macOS.appendingPathComponent("muster-daemon").path
  return FileManager.default.isExecutableFile(atPath: beside)
    ? DaemonLocation(binary: beside, data: nil, remote: remote) : nil
}

/// Where this build keeps the Linux daemons it installs on other machines: among the app's
/// resources in a bundle, where a Linux binary is a file rather than code to sign, and beside
/// the executable for a SwiftPM build, where `./dev` stages them.
private func carriedDaemons(macOS: URL) -> String? {
  let candidates = [
    macOS.deletingLastPathComponent().appendingPathComponent("Resources/daemons"),
    macOS.appendingPathComponent("daemons"),
  ]
  var isDirectory: ObjCBool = false
  return candidates.first {
    FileManager.default.fileExists(atPath: $0.path, isDirectory: &isDirectory)
      && isDirectory.boolValue
  }?.path
}
