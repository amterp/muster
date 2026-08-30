import Foundation

// Where the daemon Muster ships actually sits, which is the one part of owning it that is an
// OS and packaging question. The core starts it and talks to it; this decides that it lives
// beside the running executable, and hands the path over at startup - the same division the
// log file and the config file already draw.

/// The helper bundle a real Muster.app carries its daemon in.
///
/// `Contents/Library/` is where a bundle puts a helper application, one directory up from the
/// executable that looks for it.
private let daemonBundle = "MusterSessions.app"

/// This build's daemon, or nil if it has none. A bundle where there is one, and a bare binary
/// otherwise, because the core starts those two differently and the path is how it tells.
///
/// **The bundle is preferred because of what macOS charges a pane's protected request to.** It
/// charges the *responsible* process, a spawned child inherits its spawner's, and only a
/// process Launch Services started is its own - so a daemon spawned beside the app is charged
/// to Muster until Muster exits and to nothing nameable afterwards, while a daemon opened from
/// a bundle is charged to that bundle for as long as it lives. Since the daemon is started and
/// never stopped, that is across every relaunch. Measured, with the arrangements side by side,
/// in `docs/observations/macos-26.4.1.md`.
///
/// The bare binary beside the executable is what `./dev` stages for a plain build and what
/// every test uses, and it stays the answer there. So a dev build keeps the attribution it
/// always had, which is worth knowing when a permission behaves differently from the bundle.
///
/// Deliberately not PATH, either way: the daemon Muster starts should be the version its
/// corpus was recorded against, and the herdr somebody runs for their own work stays whatever
/// they want it to be.
///
/// `MUSTER_HERDR` overrides both, for bisecting herdr itself. The suite already spells the
/// override that way, so there is one name for it rather than two. It names a binary rather
/// than a bundle, which is the point - a bisect is not the moment to change what macOS charges
/// as well.
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

  let macOS = URL(fileURLWithPath: executable).deletingLastPathComponent()

  // Asked for as a directory rather than a file: a bundle is one, and `isExecutableFile` says
  // no to it, so the check that finds the bare binary cannot be reused here.
  let bundle = macOS.deletingLastPathComponent()
    .appendingPathComponent("Library")
    .appendingPathComponent(daemonBundle)
  var isDirectory: ObjCBool = false
  if FileManager.default.fileExists(atPath: bundle.path, isDirectory: &isDirectory),
    isDirectory.boolValue
  {
    return bundle.path
  }

  let beside = macOS.appendingPathComponent("herdr").path
  return FileManager.default.isExecutableFile(atPath: beside) ? beside : nil
}
