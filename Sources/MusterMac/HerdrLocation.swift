import Foundation

// Where the daemon Muster ships actually sits, which is the one part of owning it that is an
// OS and packaging question. The core starts it and talks to it; this decides that it lives
// beside the running executable, and hands the path over at startup - the same division the
// log file and the config file already draw.

/// This build's herdr binary, or nil if it has none.
///
/// Beside the executable, because that is where a bundle puts its helpers and where `./dev`
/// stages them for a plain build - one rule covering both, and the same one `PaneCommand`
/// uses to find muster-bridge. Deliberately not PATH: the daemon Muster starts should be the
/// version its corpus was recorded against, and the herdr somebody runs for their own work
/// stays whatever they want it to be.
///
/// `MUSTER_HERDR` overrides, for bisecting herdr itself. The suite already spells the
/// override that way, so there is one name for it rather than two.
///
/// Nil means this build staged no daemon. A real state rather than a default to paper over -
/// the core reports it as a window with nothing behind it, which is what it is.
/// Takes the environment rather than reading it, the way the core's socket discovery does, so
/// the rules are answerable without one - and so a developer who exports `MUSTER_HERDR` for
/// their own work does not change what the suite is testing.
public func herdrPath(
  executable: String,
  environment: [String: String] = ProcessInfo.processInfo.environment
) -> String? {
  if let explicit = environment["MUSTER_HERDR"], !explicit.isEmpty {
    return explicit
  }

  let beside = URL(fileURLWithPath: executable)
    .deletingLastPathComponent()
    .appendingPathComponent("herdr")
    .path
  return FileManager.default.isExecutableFile(atPath: beside) ? beside : nil
}
