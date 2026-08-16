import Foundation

// Where the config file lives, which is the one part of configuration that is an OS question.
// The core reads it and decides what it means; this file decides it is `config.toml` in Muster's
// own home, and hands the path over at startup. A port picks its own answer here and nothing
// below changes.

/// This run's config file, if there is one.
///
/// `~/.muster/config.toml`, moved by `MUSTER_HOME`, and overridable outright with `MUSTER_CONFIG`
/// so a test or a bug report can point at a file of its own.
///
/// Nil when nothing is there, which is the ordinary case rather than a problem: a Muster with no
/// config file finds the daemon on this machine the way herdr's own client would. The distinction
/// that matters is between absent and unreadable, and it is the core that draws it - a path
/// handed over that turns out to be unparseable is worth a line saying so, where a path that was
/// never there is worth nothing.
///
/// `exists` is a parameter for the reason `environment` is: asking the real filesystem is the one
/// thing here a test cannot say anything about, and it is also the only thing that makes this
/// function's answer depend on the machine it runs on.
public func configPath(
  environment: [String: String] = ProcessInfo.processInfo.environment,
  exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }
) -> String? {
  if let explicit = environment["MUSTER_CONFIG"], !explicit.isEmpty {
    return explicit
  }
  guard let path = configuredPath(environment: environment) else { return nil }
  return exists(path) ? path : nil
}

/// Where a config file would go, whether or not one is there.
private func configuredPath(environment: [String: String]) -> String? {
  musterHome(environment: environment)?.appendingPathComponent("config.toml").path
}

/// The config file left behind at the pre-`~/.muster` path, if that is what happened.
///
/// Muster does not move a person's file, so somebody who had one gets a window with none of their
/// daemons in it and nothing to explain why. That failure looks exactly like a daemon being down,
/// which is the reason this is worth a line rather than a release note.
///
/// Transitional, and deliberately so: delete this and its caller once nobody is starting from a
/// Muster old enough to have written the old path.
public func strandedConfigPath(
  environment: [String: String] = ProcessInfo.processInfo.environment,
  exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }
) -> String? {
  // Only when the new path holds nothing. Someone who has already moved theirs has two files by
  // choice, and telling them about the one they left is noise.
  if configPath(environment: environment, exists: exists) != nil { return nil }

  let base: URL
  if let xdg = environment["XDG_CONFIG_HOME"], !xdg.isEmpty {
    base = URL(fileURLWithPath: xdg, isDirectory: true)
  } else if let home = environment["HOME"], !home.isEmpty {
    base = URL(fileURLWithPath: home, isDirectory: true)
      .appendingPathComponent(".config", isDirectory: true)
  } else {
    return nil
  }

  let old = base.appendingPathComponent("muster/config.toml").path
  return exists(old) ? old : nil
}
