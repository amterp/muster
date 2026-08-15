import Foundation

// Where the config file lives, which is the one part of configuration that is an OS
// question. The core reads it and decides what it means; this file decides it belongs in
// ~/.config/muster, and hands the path over at startup. A port picks its own answer here and
// nothing below changes.

/// This run's config file, if there is one.
///
/// `~/.config/muster/config.toml`, honouring `XDG_CONFIG_HOME`, and overridable with
/// `MUSTER_CONFIG` so a test or a bug report can point at a file of its own.
///
/// Nil when nothing is there, which is the ordinary case rather than a problem: a Muster with
/// no config file finds the daemon on this machine the way herdr's own client would. The
/// distinction that matters is between absent and unreadable, and it is the core that draws
/// it - a path handed over that turns out to be unparseable is worth a line saying so, where
/// a path that was never there is worth nothing.
public func configPath() -> String? {
  let environment = ProcessInfo.processInfo.environment
  if let explicit = environment["MUSTER_CONFIG"], !explicit.isEmpty {
    return explicit
  }

  let base: URL
  if let xdg = environment["XDG_CONFIG_HOME"], !xdg.isEmpty {
    base = URL(fileURLWithPath: xdg, isDirectory: true)
  } else {
    base = FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent(".config", isDirectory: true)
  }

  let path = base.appendingPathComponent("muster/config.toml").path
  return FileManager.default.fileExists(atPath: path) ? path : nil
}
