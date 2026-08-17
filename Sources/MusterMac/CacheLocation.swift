import Foundation

/// Where Muster keeps what it had to download.
///
/// One thing needs it: the daemon on a machine that is not this one. Muster runs the herdr it
/// pinned rather than whatever is installed over there, and a build carries a binary for this
/// platform only - so attaching a devenv means fetching that machine's release asset and pushing
/// it across. Fetched once and kept, because the alternative is 18 MB on every launch.
///
/// Under Muster's own home rather than in `~/Library/Caches` or an XDG cache tree, following the
/// rule the rest of this directory follows: one place holds everything Muster owns, and
/// `MUSTER_HOME` moves the lot. That is also what lets a test point at a scratch home and get a
/// scratch cache without being told about this file.
///
/// `nil` when the environment says nothing about where home is. A real state rather than a
/// failure: the download then goes to a temporary that is thrown away, so a machine with nowhere
/// to cache is slow to attach a devenv rather than unable to.
public func cachePath(environment: [String: String] = ProcessInfo.processInfo.environment)
  -> String?
{
  musterHome(environment: environment)?.appendingPathComponent("cache").path
}
