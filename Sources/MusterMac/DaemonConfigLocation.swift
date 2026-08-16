import Foundation

/// Where the daemon's own config is written, translated from Muster's.
///
/// The backend's half of the arrangement the renderer already has, and it exists for the same
/// reason: herdr takes a value only as a file. Muster derives one from `~/.muster/config.toml`
/// and names it with `HERDR_CONFIG_PATH`, which moves the file the daemon reads without moving
/// the socket it binds - so what a pane runs and how deep its scrollback is become questions
/// Muster's own file answers, and the daemon Muster pinned by checksum stops taking its update
/// policy from a file Muster does not own.
///
/// State rather than configuration, beside `window.toml` and `libghostty.conf` for the same
/// reason: Muster writes it, nobody should edit it, and editing it changes nothing because the
/// next launch overwrites it. Worth writing where a person can find it, because it is the
/// answer to "what did Muster actually tell the daemon" - the first question when a pane opens
/// the wrong shell.
///
/// Always answers, on the same terms as the renderer's. A daemon handed no file falls back to
/// the user's own herdr config, which is a different session from the one somebody configured,
/// so somewhere is always better than refusing.
public func daemonConfigPath(environment: [String: String] = ProcessInfo.processInfo.environment)
  -> String
{
  if let home = musterHome(environment: environment) {
    return home.appendingPathComponent("state/herdr.toml").path
  }
  return URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
    .appendingPathComponent("muster-herdr.toml").path
}
