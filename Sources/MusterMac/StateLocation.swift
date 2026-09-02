import Foundation

/// Where a window's arrangement is remembered.
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
/// One file per window rather than one for all of them, and that is what makes a window a thing
/// that can be closed and brought back. While there was one file, two windows read and wrote it
/// in turn and whichever published last decided what came back; a window that closed left nothing
/// behind that named it.
public enum Arrangements {
  /// The directory the records live in.
  public static func directory(environment: [String: String]) -> URL? {
    musterHome(environment: environment)?.appendingPathComponent("state/windows", isDirectory: true)
  }

  /// The arrangement this launch adopts, and the claim it leaves on it while it runs.
  ///
  /// Three answers, in order. `MUSTER_STATE` names a file outright and takes no claim, which is
  /// what a test and a script want. A launch somebody asked for takes a record nothing has ever
  /// held. Anything else takes the most recently written record no live window is holding, which
  /// is the window Muster comes back to - and, when another window is running, the one that was
  /// closed.
  ///
  /// `nil` is a real answer and not a failure: the window opens fresh and remembers nothing,
  /// which is what every window did before any of this existed.
  public static func open(
    fresh: Bool,
    environment: [String: String] = ProcessInfo.processInfo.environment,
    pid: Int32 = ProcessInfo.processInfo.processIdentifier
  ) -> String? {
    if let explicit = environment["MUSTER_STATE"] {
      // Deliberately including empty, which is how a test or a script says "remember nothing"
      // rather than "look in the usual place".
      return explicit.isEmpty ? nil : explicit
    }
    guard let directory = directory(environment: environment) else { return nil }
    try? FileManager.default.createDirectory(
      at: directory, withIntermediateDirectories: true)

    // Before anything is chosen, so that a record whose window was killed is available again
    // rather than held by a process that is gone.
    releaseDeadClaims(in: directory)
    adoptTheOldSingleFile(into: directory, environment: environment)

    let record = fresh ? mint(in: directory) : (free(in: directory) ?? mint(in: directory))
    claim(record, by: pid)
    return record.path
  }

  /// Says the window holding this record has gone, so the next launch may take it.
  ///
  /// Called on the way out. Not relied on: a window that is killed never gets here, which is why
  /// a claim carries a pid and `releaseDeadClaims` checks it. What this buys is the case in
  /// between - quit one window and reopen it in the same second, before anything has swept.
  public static func release(_ path: String) {
    try? FileManager.default.removeItem(at: claimFile(for: URL(fileURLWithPath: path)))
  }

  /// One window's slot: the arrangement it writes, and the claim a live window leaves on it.
  ///
  /// Both halves are keyed by the same stem, because a window claims its slot at launch and the
  /// core writes the arrangement into it later - so between those two moments the slot exists
  /// with no file in it, and a launch that only looked at files would hand the same one out
  /// twice.
  struct Slot {
    let stem: String
    let record: URL
    let held: Bool
    /// When the arrangement was last written, or nothing when none has been.
    let written: Date?
  }

  /// Every slot in the directory, newest arrangement first.
  static func slots(in directory: URL) -> [Slot] {
    let names =
      (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
    var stems = Set<String>()
    for name in names where name.hasSuffix(".toml") || name.hasSuffix(".held") {
      stems.insert(String(name.dropLast(5)))
    }
    return
      stems
      .map { stem -> Slot in
        let record = directory.appendingPathComponent("\(stem).toml")
        return Slot(
          stem: stem,
          record: record,
          held: FileManager.default.fileExists(atPath: claimFile(for: record).path),
          written: written(record))
      }
      .sorted { ($0.written ?? .distantPast) > ($1.written ?? .distantPast) }
  }

  /// The most recent slot no window is holding and something has actually been written into.
  ///
  /// A slot with no arrangement in it is skipped rather than adopted: it belongs to a window
  /// that claimed one and quit before publishing, so there is nothing there to come back to and
  /// taking it would look like a window that forgot everything.
  private static func free(in directory: URL) -> URL? {
    slots(in: directory).first { !$0.held && $0.written != nil }?.record
  }

  /// A slot nothing is holding and nothing has been written into.
  ///
  /// Numbered rather than named from the registry that mints pane and tab names: that registry
  /// is the core's, and the core is not running yet - which file to hand it is the question
  /// being answered here. A number is what a person would call these anyway.
  ///
  /// The oldest are dropped once there are more than `kept`. A record is a few hundred bytes, so
  /// this is about a directory somebody opens rather than about space.
  private static func mint(in directory: URL) -> URL {
    let existing = slots(in: directory)
    for old in existing.dropFirst(kept - 1) where !old.held {
      try? FileManager.default.removeItem(at: old.record)
      try? FileManager.default.removeItem(at: claimFile(for: old.record))
    }
    let taken = Set(slots(in: directory).filter { $0.held || $0.written != nil }.map(\.stem))
    var number = 1
    while taken.contains("window-\(number)") { number += 1 }
    return directory.appendingPathComponent("window-\(number).toml")
  }

  /// How many closed windows are worth being able to reopen.
  private static let kept = 20

  private static func claim(_ record: URL, by pid: Int32) {
    try? String(pid).write(to: claimFile(for: record), atomically: true, encoding: .utf8)
  }

  private static func claimFile(for record: URL) -> URL {
    record.deletingPathExtension().appendingPathExtension("held")
  }

  /// Drops the claims of windows that are no longer running.
  ///
  /// A pid that no longer exists is the test, on the same terms as the endpoint sockets:
  /// `kill(pid, 0)` reports existence without sending anything, and EPERM counts as alive, since
  /// a process owned by somebody else is still a process.
  private static func releaseDeadClaims(in directory: URL) {
    let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
    for name in names where name.hasSuffix(".held") {
      let claim = directory.appendingPathComponent(name)
      guard let digits = try? String(contentsOf: claim, encoding: .utf8),
        let pid = pid_t(digits.trimmingCharacters(in: .whitespacesAndNewlines))
      else {
        try? FileManager.default.removeItem(at: claim)
        continue
      }
      if kill(pid, 0) == 0 || errno == EPERM { continue }
      try? FileManager.default.removeItem(at: claim)
    }
  }

  /// Moves the one file every window used to share into the first record.
  ///
  /// A rename rather than a read, so the arrangement somebody had when they upgraded is the one
  /// their window comes back to. Only ever fires once: afterwards there is no file to move.
  private static func adoptTheOldSingleFile(into directory: URL, environment: [String: String]) {
    guard let home = musterHome(environment: environment) else { return }
    let old = home.appendingPathComponent("state/window.toml")
    guard FileManager.default.fileExists(atPath: old.path) else { return }
    let first = directory.appendingPathComponent("window-1.toml")
    guard !FileManager.default.fileExists(atPath: first.path) else { return }
    try? FileManager.default.moveItem(at: old, to: first)
  }

  /// When an arrangement was last written, or nothing when the file is not there yet.
  private static func written(_ url: URL) -> Date? {
    guard FileManager.default.fileExists(atPath: url.path) else { return nil }
    return try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
  }
}

/// Where what Muster calls each pane is remembered.
///
/// Beside the arrangements, because both are Muster's own state and neither is anything a person
/// should have to edit. One file for all of them, unlike an arrangement, because names belong to
/// the panes rather than to a window: every window calls a pane the same thing, and the panes
/// outlive all of them.
///
/// Nowhere to write is a real answer - names then last one launch, and a pane open across a
/// restart can no longer say which pane it is.
public func paneNamesPath(environment: [String: String] = ProcessInfo.processInfo.environment)
  -> String?
{
  if let explicit = environment["MUSTER_PANE_NAMES"] {
    return explicit.isEmpty ? nil : explicit
  }
  guard let home = musterHome(environment: environment) else { return nil }
  return home.appendingPathComponent("state/panes.toml").path
}
