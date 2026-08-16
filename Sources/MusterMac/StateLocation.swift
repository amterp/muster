import Foundation

/// Where this window's arrangement is remembered.
///
/// State rather than configuration, and the two stay separate on purpose: a config file is
/// something a person writes and would be annoyed to find rewritten, and this is something Muster
/// writes and nobody should have to edit. They share a home now, so the separation is a `state/`
/// directory rather than a different tree - which is the honest shape, because both belong to
/// Muster and only one belongs to the person.
///
/// An OS question, which is why it is answered here and handed to the core at startup - the same
/// division the log file and the config file already draw.
///
/// Takes its environment as a parameter so a test says what it is testing, rather than depending
/// on what the developer running it happens to have exported.
public func statePath(environment: [String: String] = ProcessInfo.processInfo.environment)
  -> String?
{
  if let explicit = environment["MUSTER_STATE"] {
    // Deliberately including empty, which is how a test or a script says "remember nothing"
    // rather than "look in the usual place".
    return explicit.isEmpty ? nil : explicit
  }

  // Nowhere to write is a real answer: the window opens fresh, which is what it did before any
  // of this existed.
  guard let home = musterHome(environment: environment) else { return nil }
  return home.appendingPathComponent("state/window.toml").path
}
