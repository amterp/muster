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
    named: String? = nil,
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

    if let record = reopening(named, in: directory), claim(record, by: pid) {
      return record.path
    }
    // Chosen again whenever another launch claims the same slot first: the claim that beat this
    // one now marks the slot held, so the next choice is a different one.
    for _ in 0..<claimAttempts {
      let record = fresh ? mint(in: directory) : (free(in: directory) ?? mint(in: directory))
      if claim(record, by: pid) { return record.path }
    }
    return nil
  }

  /// How many slots a launch tries before opening a window that remembers nothing. Only a launch
  /// racing this many others at once ever reaches the last.
  private static let claimAttempts = 8

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

  /// The slot a launch was told to reopen, if no live window is holding it.
  ///
  /// Held means that window is open after all - it opened between somebody going to its tab and
  /// this launch - and taking its record as well would be two windows writing one file. The
  /// launch then goes the ordinary way instead.
  private static func reopening(_ name: String?, in directory: URL) -> URL? {
    guard let name, !name.contains("/") else { return nil }
    return slots(in: directory).first { $0.stem == name && !$0.held }?.record
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

  /// Claims a slot for a window, and says whether it got it.
  ///
  /// Exclusive: two launches that both found a slot free cannot both claim it, because the claim
  /// is linked into place and a link onto a name that exists fails. Written beside it first, so
  /// nobody ever reads a claim with no pid in it and sweeps it as garbage.
  ///
  /// `linking` is `link(2)`, and a parameter so that a test can stand in for a filesystem that
  /// cannot link.
  static func claim(
    _ record: URL, by pid: Int32,
    linking: (String, String) -> Int32 = { link($0, $1) }
  ) -> Bool {
    let claim = claimFile(for: record)
    let written = claim.appendingPathExtension("\(pid)")
    guard (try? String(pid).write(to: written, atomically: false, encoding: .utf8)) != nil else {
      return false
    }
    defer { unlink(written.path) }
    if linking(written.path, claim.path) == 0 { return true }
    let refused = errno
    if refused == EEXIST { return false }
    // A filesystem that will not link at all. Still exclusive, but the claim exists empty for a
    // moment before its pid is in it, which a launch sweeping dead claims at that moment reads as
    // garbage - a narrow race, and better than every window remembering nothing.
    linkRefused = String(cString: strerror(refused))
    return createExclusively(claim, holding: pid)
  }

  /// Why the last claim could not be linked into place, when that happened.
  ///
  /// Kept rather than logged, because a claim is made before the core that writes the log has
  /// started. The launch says it once the core is up. Unsynchronized because a launch claims once,
  /// on the main thread, before anything else is running.
  nonisolated(unsafe) public private(set) static var linkRefused: String?

  private static func createExclusively(_ claim: URL, holding pid: Int32) -> Bool {
    let descriptor = Darwin.open(claim.path, O_WRONLY | O_CREAT | O_EXCL, 0o644)
    guard descriptor >= 0 else { return false }
    defer { close(descriptor) }
    let text = String(pid)
    let wrote = text.withCString { Darwin.write(descriptor, $0, strlen($0)) }
    return wrote == text.utf8.count
  }

  private static func claimFile(for record: URL) -> URL {
    record.deletingPathExtension().appendingPathExtension("held")
  }

  /// Drops the claims of windows that are no longer running.
  ///
  /// A pid that no longer exists is the first test, on the same terms as the endpoint sockets:
  /// `kill(pid, 0)` reports existence without sending anything, and EPERM counts as alive, since
  /// a process owned by somebody else is still a process.
  ///
  /// A pid that exists is not enough, because macOS hands a dead window's pid to the next process
  /// that needs one, and a claim that looks alive forever is a window nobody can reopen. The
  /// process that wrote a claim started before it wrote it, so one that started after the claim
  /// was written is somebody else.
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
      let exists = kill(pid, 0) == 0 || errno == EPERM
      if exists, !startedAfter(pid, claim) { continue }
      try? FileManager.default.removeItem(at: claim)
    }
  }

  /// Whether the process with this pid started after the claim was written.
  ///
  /// False when either time cannot be read, which keeps the claim: a window wrongly thought
  /// closed is two windows writing one record, and that is the worse mistake.
  ///
  /// With two seconds' slack, because HFS+ keeps a file's time to the second: a window started at
  /// .3 that wrote its claim at .8 has a claim that reads as written before it started. A pid
  /// reused that soon after the window that held it wrote its claim is not a case that happens.
  private static func startedAfter(_ pid: pid_t, _ claim: URL) -> Bool {
    guard let started = started(pid), let written = self.written(claim) else { return false }
    return started > written.addingTimeInterval(2)
  }

  /// When a process started, or nothing when there is no such process.
  static func started(_ pid: pid_t) -> Date? {
    var process = kinfo_proc()
    var size = MemoryLayout<kinfo_proc>.stride
    var name: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
    guard sysctl(&name, 4, &process, &size, nil, 0) == 0,
      size == MemoryLayout<kinfo_proc>.stride
    else { return nil }
    let start = process.kp_proc.p_un.__p_starttime
    return Date(
      timeIntervalSince1970: TimeInterval(start.tv_sec) + TimeInterval(start.tv_usec) / 1_000_000)
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

/// Where every window writes which window holds each tab.
///
/// One file for all of them, like the names beside it, because the rule it keeps spans windows: a
/// tab belongs to exactly one, and a window lists only its own. In a directory of its own, because
/// the shell watches it for another window's changes and should not wake for every arrangement a
/// window saves.
///
/// Nowhere to write is a real answer - the window then holds every tab, as a single window always
/// did.
public func tabHoldersPath(environment: [String: String] = ProcessInfo.processInfo.environment)
  -> String?
{
  if let explicit = environment["MUSTER_TAB_HOLDERS"] {
    return explicit.isEmpty ? nil : explicit
  }
  guard let home = musterHome(environment: environment) else { return nil }
  return home.appendingPathComponent("state/holding/tabs.toml").path
}
