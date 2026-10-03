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
  /// The directory the records live in: `windows/` in this install's own state directory, which
  /// the claim on the app answered (`InstallState`).
  public static func directory(state: URL?) -> URL? {
    state?.appendingPathComponent("windows", isDirectory: true)
  }

  /// The arrangement a window opens onto.
  ///
  /// Three answers, in order. `MUSTER_STATE` names a file outright, which is what a test and a
  /// script want. A window somebody asked for takes a record nothing has been written into. Any
  /// other takes the most recently written record no window here has open, which is the window
  /// Muster comes back to - and, when another window is open, the one closed last.
  ///
  /// `open` is the arrangements this app's windows have open. Every window of the install is a
  /// window of this app, so no other process can hold one (mip/0006-one-process.md, section 5).
  ///
  /// `nil` is a real answer and not a failure: the window opens fresh and remembers nothing,
  /// which is what every window did before any of this existed.
  public static func open(
    fresh: Bool,
    named: String? = nil,
    environment: [String: String] = ProcessInfo.processInfo.environment,
    state: URL? = InstallState.directory,
    open: Set<String> = []
  ) -> String? {
    if let explicit = environment["MUSTER_STATE"] {
      // Deliberately including empty, which is how a test or a script says "remember nothing"
      // rather than "look in the usual place".
      return explicit.isEmpty ? nil : explicit
    }
    guard let directory = directory(state: state) else { return nil }
    try? FileManager.default.createDirectory(
      at: directory, withIntermediateDirectories: true)
    adoptTheOldSingleFile(into: directory, environment: environment)

    let slots = slots(in: directory, open: open)
    if let named, !named.contains("/"),
      let slot = slots.first(where: { $0.stem == named && !$0.open })
    {
      return slot.record.path
    }
    let record =
      fresh ? mint(in: directory, slots: slots) : (free(slots) ?? mint(in: directory, slots: slots))
    return record.path
  }

  /// One window's record, and whether a window here has it open.
  struct Slot {
    let stem: String
    let record: URL
    let open: Bool
    /// When the arrangement was last written, or nothing when none has been.
    let written: Date?
  }

  /// Every record in the directory, newest first, with the ones in `open` marked.
  static func slots(in directory: URL, open: Set<String> = []) -> [Slot] {
    let names =
      (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
    var stems = Set(names.filter { $0.hasSuffix(".toml") }.map { String($0.dropLast(5)) })
    // A window here that has not written its record yet still has it.
    for path in open {
      let record = URL(fileURLWithPath: path)
      if record.deletingLastPathComponent().standardizedFileURL == directory.standardizedFileURL {
        stems.insert(record.deletingPathExtension().lastPathComponent)
      }
    }
    let openPaths = Set(open.map { URL(fileURLWithPath: $0).standardizedFileURL.path })
    return
      stems
      .map { stem -> Slot in
        let record = directory.appendingPathComponent("\(stem).toml")
        return Slot(
          stem: stem,
          record: record,
          open: openPaths.contains(record.standardizedFileURL.path),
          written: written(record))
      }
      .sorted { ($0.written ?? .distantPast) > ($1.written ?? .distantPast) }
  }

  /// The most recent record no window here has open and something has actually been written
  /// into.
  ///
  /// A record with nothing written in it is skipped rather than adopted: it belongs to a window
  /// that opened and quit before publishing, so there is nothing there to come back to and taking
  /// it would look like a window that forgot everything.
  private static func free(_ slots: [Slot]) -> URL? {
    slots.first { !$0.open && $0.written != nil }?.record
  }

  /// A record no window here has open and nothing has been written into.
  ///
  /// Numbered rather than named from the registry that mints pane and tab names: that registry
  /// is the core's, and which file to hand it is the question being answered here. A number is
  /// what a person would call these anyway.
  ///
  /// The oldest are dropped once there are more than `kept`. A record is a few hundred bytes, so
  /// this is about a directory somebody opens rather than about space.
  private static func mint(in directory: URL, slots: [Slot]) -> URL {
    for old in slots.dropFirst(kept - 1) where !old.open {
      try? FileManager.default.removeItem(at: old.record)
    }
    let taken = Set(slots.filter { $0.open || $0.written != nil }.map(\.stem))
    var number = 1
    while taken.contains("window-\(number)") { number += 1 }
    return directory.appendingPathComponent("window-\(number).toml")
  }

  /// How many closed windows are worth being able to reopen.
  private static let kept = 20

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
  static func written(_ url: URL) -> Date? {
    guard FileManager.default.fileExists(atPath: url.path) else { return nil }
    return try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
  }
}

/// Where every window writes which window holds each tab.
///
/// One file for all of them, because the rule it keeps spans windows: a tab belongs to exactly
/// one, and a window lists only its own. In a directory of its own, because the shell watches it
/// for another window's changes and should not wake for every arrangement a window saves.
///
/// Nowhere to write is a real answer - the window then holds every tab, as a single window always
/// did.
public func tabHoldersPath(
  environment: [String: String] = ProcessInfo.processInfo.environment,
  state: URL? = InstallState.directory
) -> String? {
  if let explicit = environment["MUSTER_TAB_HOLDERS"] {
    return explicit.isEmpty ? nil : explicit
  }
  return state?.appendingPathComponent("holding/tabs.toml").path
}

/// This install's own state directory under the Muster home: `state/<install>/`, where its window
/// arrangements and its record of which window holds each tab are kept (mip/0006-one-process.md,
/// section 5).
///
/// Set once, from the claim on the app, before anything asks. The shell cannot name it itself,
/// because the install is the core's to know.
public enum InstallState {
  nonisolated(unsafe) public static var directory: URL?
}
