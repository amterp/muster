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
  if let explicit = environment["MUSTER_DAEMON_BINARY"], !explicit.isEmpty {
    return DaemonLocation(binary: explicit, data: nil)
  }

  let macOS = URL(fileURLWithPath: executable).deletingLastPathComponent()

  let contents = macOS.deletingLastPathComponent()
    .appendingPathComponent("Library")
    .appendingPathComponent(daemonBundle)
    .appendingPathComponent("Contents")
  let bundled = contents.appendingPathComponent("MacOS").appendingPathComponent("muster-daemon")
  if FileManager.default.isExecutableFile(atPath: bundled.path) {
    return DaemonLocation(
      binary: bundled.path,
      data: contents.appendingPathComponent("Resources")
        .appendingPathComponent("muster-daemon-data").path)
  }

  let beside = macOS.appendingPathComponent("muster-daemon").path
  return FileManager.default.isExecutableFile(atPath: beside)
    ? DaemonLocation(binary: beside, data: nil) : nil
}
