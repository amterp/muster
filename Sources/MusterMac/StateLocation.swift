import Foundation

/// Where this window's arrangement is remembered.
///
/// State rather than configuration, and the two go in different places on purpose: a config
/// file is something a person writes and would be annoyed to find rewritten, and this is
/// something Muster writes and nobody should have to edit. XDG draws the same line, and the
/// state directory is where a file that is regenerated on every change belongs.
///
/// An OS question, which is why it is answered here and handed to the core at startup - the
/// same division the log file and the config file already draw.
///
/// Takes its environment as a parameter so a test says what it is testing, rather than
/// depending on what the developer running it happens to have exported.
public func statePath(environment: [String: String] = ProcessInfo.processInfo.environment)
  -> String?
{
  if let explicit = environment["MUSTER_STATE"] {
    // Deliberately including empty, which is how a test or a script says "remember nothing"
    // rather than "look in the usual place".
    return explicit.isEmpty ? nil : explicit
  }

  let base: URL
  if let xdg = environment["XDG_STATE_HOME"], !xdg.isEmpty {
    base = URL(fileURLWithPath: xdg, isDirectory: true)
  } else if let home = environment["HOME"], !home.isEmpty {
    base = URL(fileURLWithPath: home, isDirectory: true)
      .appendingPathComponent(".local/state", isDirectory: true)
  } else {
    // Nowhere to write is a real answer: the window opens fresh, which is what it did before
    // any of this existed.
    return nil
  }

  return base.appendingPathComponent("muster/window.toml").path
}
