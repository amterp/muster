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

/// The same daemon, as something a process can exec.
///
/// Two callers want different answers to "which herdr", and the difference is not cosmetic.
/// The core wants what [`herdrPath`] returns, because it starts a bundle through Launch
/// Services and a bare binary by spawning it, and reads the path to decide which. A bridge
/// runs `herdr terminal session control` and can only exec a Mach-O, so it wants the binary
/// inside.
///
/// Derived here rather than worked out by the bridge, which is the whole of kan a_2Hnh3g0Y5:
/// the bridge used to look for a file called `herdr` beside its own executable, that rule was
/// right until the daemon moved into the helper bundle, and nothing made the two answers move
/// together. A released cask then rendered nothing in every pane, and the PATH fallback could
/// not save it - an app opened by Launch Services gets launchd's
/// `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, every entry SIP-protected, so there is nowhere to put
/// a herdr even deliberately.
///
/// A path ending in `.app` is read as a bundle, the same test `muster_herdr::daemon::launch`
/// applies to the same value on the other side of the seam.
public func herdrBinaryPath(
  executable: String,
  environment: [String: String] = ProcessInfo.processInfo.environment
) -> String? {
  guard let daemon = herdrPath(executable: executable, environment: environment) else {
    return nil
  }
  let path = URL(fileURLWithPath: daemon)
  guard path.pathExtension.lowercased() == "app" else {
    return daemon
  }
  // `CFBundleExecutable` in the plist `./dev` writes for this bundle. Named rather than
  // discovered because the plist is not there to be read on the one path that matters: a
  // bridge is spawned per pane, and re-reading a bundle's metadata fifteen times to learn a
  // name this repo writes itself is work nobody asked for.
  return
    path
    .appendingPathComponent("Contents")
    .appendingPathComponent("MacOS")
    .appendingPathComponent("herdr")
    .path
}
