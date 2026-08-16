import Foundation

/// Where the renderer's own config is written, translated from Muster's.
///
/// libghostty has no setter - the only way to give it a value is to hand it a file - so Muster
/// derives one from `~/.muster/config.toml` on every launch
/// (docs/observations/libghostty-9f9b8d1d.md section 9). State rather than configuration, and
/// beside `window.toml` for the same reason: Muster writes it, nobody should edit it, and
/// editing it would change nothing because the next launch overwrites it.
///
/// Worth writing somewhere a person can find rather than into a temporary directory. It is the
/// answer to "what did Muster actually tell the renderer", which is the first question when a
/// colour does not take.
///
/// Always answers, unlike the config and state paths. Those have a "nowhere" that means
/// something - no config file, no remembered layout - and this one does not: a renderer handed
/// nothing paints its own defaults, which is a different window from the one somebody
/// configured, so somewhere is always better than refusing.
public func rendererConfigPath(environment: [String: String] = ProcessInfo.processInfo.environment)
  -> String
{
  if let home = musterHome(environment: environment) {
    return home.appendingPathComponent("state/libghostty.conf").path
  }
  return URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
    .appendingPathComponent("muster-libghostty.conf").path
}
