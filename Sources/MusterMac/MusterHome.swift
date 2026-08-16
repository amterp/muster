import Foundation

/// Where Muster keeps everything of its own.
///
/// One directory rather than a file in each of the XDG trees. Muster's surface is meant to be
/// discovered rather than taught - "configuration is files" is a desideratum, and an agent that
/// can list one directory needs no documentation to find the whole of it. Three trees is the
/// arrangement that made a person hunt.
///
/// It follows that `XDG_CONFIG_HOME` and friends no longer move Muster's own files. They still
/// decide where *herdr* listens and what config *herdr* reads, because those are herdr's rules
/// and Muster only passes them on (`crates/muster-herdr/src/daemon.rs`).
///
/// An OS question, which is why it is answered here and handed to the core at startup - the same
/// division the log file and the daemon binary already draw.
///
/// Takes its environment as a parameter so a test says what it is testing, rather than depending
/// on what the developer running it happens to have exported.
public func musterHome(environment: [String: String] = ProcessInfo.processInfo.environment) -> URL?
{
  if let explicit = environment["MUSTER_HOME"], !explicit.isEmpty {
    return URL(fileURLWithPath: explicit, isDirectory: true)
  }
  guard let home = environment["HOME"], !home.isEmpty else {
    // Nowhere to look is a real answer, and the callers have one each: a window that opens
    // fresh, and a Muster that finds the daemon on this machine for itself. Building a path
    // from an empty base would name something in the filesystem root instead.
    return nil
  }
  return URL(fileURLWithPath: home, isDirectory: true)
    .appendingPathComponent(".muster", isDirectory: true)
}
