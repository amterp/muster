import Foundation

// Where the commands Muster ships live, and how a pane comes to find them.
//
// An agent in a pane cannot drive the window it is drawn in if `muster` is not on its PATH, and
// asking anybody to put it there would make the surface something you have to be taught rather
// than something you find. So Muster keeps a directory of its own, points a link in it at the CLI
// this build staged, and hands the directory to the core - which puts it on the PATH of every
// daemon it starts, and so of every pane those daemons spawn.
//
// A link rather than a copy, refreshed at every launch: the app it points at moves - a build to a
// bundle, one version to the next - and a copy would be a stale `muster` talking to a window it no
// longer matches. Following the same division as every path here: where these things live is an OS
// question, and what to do with them is the core's.

/// The directory Muster keeps its own commands in.
///
/// Beside `state/`, and a sibling of it rather than inside it, because these two things belong to
/// different people: nothing under `state/` is anybody's business but Muster's, and this is a
/// directory a person may well want on their own PATH.
public func commandsPath(environment: [String: String] = ProcessInfo.processInfo.environment)
  -> String?
{
  if let explicit = environment["MUSTER_COMMANDS"] {
    // Empty is how a test or a script says "put nothing on anyone's PATH", the same spelling
    // every other path here uses.
    return explicit.isEmpty ? nil : explicit
  }
  guard let home = musterHome(environment: environment) else { return nil }
  return home.appendingPathComponent("bin", isDirectory: true).path
}

/// Points `<commands>/muster` at the CLI this build staged, and says whether a pane will find one.
///
/// Nil means there is no command to offer, and then the core is told nothing: an empty directory on
/// a daemon's PATH is harmless, but a *dangling* link on it is worse than nothing - `muster` would
/// exist, fail to exec, and look like a broken install rather than an absent one. So a link left by
/// a build whose binary has since gone is taken away.
///
/// Beside the running executable, because that is where a bundle puts its helpers and where `./dev`
/// stages them - the same one rule `herdrPath` and `PaneCommand` already follow. The staged name is
/// `muster-cli` because the app's own executable is already called `muster`; the link is what gives
/// it the name people type.
@discardableResult
public func refreshMusterCommand(
  executable: String,
  commands: String?,
  files: FileManager = .default
) -> String? {
  guard let commands else { return nil }
  let link = URL(fileURLWithPath: commands, isDirectory: true)
    .appendingPathComponent("muster")
  let staged = URL(fileURLWithPath: executable)
    .deletingLastPathComponent()
    .appendingPathComponent("muster-cli")

  guard files.isExecutableFile(atPath: staged.path) else {
    // Only a link is removed, and only one that no longer resolves. Anything else in this
    // directory belongs to whoever put it there.
    if let existing = try? files.destinationOfSymbolicLink(atPath: link.path),
      !files.isExecutableFile(atPath: absolute(existing, from: link))
    {
      try? files.removeItem(at: link)
    }
    return nil
  }

  do {
    try files.createDirectory(
      at: link.deletingLastPathComponent(), withIntermediateDirectories: true)
    // Removed and remade rather than checked and left, because the answer to "does this point at
    // the right thing" is the same amount of work as pointing it there.
    if (try? files.destinationOfSymbolicLink(atPath: link.path)) != nil
      || files.fileExists(atPath: link.path)
    {
      try files.removeItem(at: link)
    }
    try files.createSymbolicLink(at: link, withDestinationURL: staged)
  } catch {
    return nil
  }
  return commands
}

/// A symlink's destination as a path, since it may have been written relative to the link.
private func absolute(_ destination: String, from link: URL) -> String {
  if destination.hasPrefix("/") { return destination }
  return link.deletingLastPathComponent().appendingPathComponent(destination).path
}
