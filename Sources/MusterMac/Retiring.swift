import AppKit

/// Window processes from before every window of an install was a window of one app, asked to quit
/// so that their windows reopen here (mip/0006-one-process.md, section 6).
///
/// Such a process still runs a window of its own after an upgrade that did not quit it, or a
/// launch while it was open. Left running, it would show tabs this app also shows, and nothing
/// lets the two agree any more. Quitting it costs nothing an ordinary quit does not: its
/// arrangement is written as it settles, and the daemon keeps every pane.
///
/// It is found by the claim it left on its arrangement in the directory every install used to
/// share, `state/windows/`, which is how a process from before said which window it was. Only a
/// process of this same bundle is asked: one of another bundle, such as another checkout's
/// development build, follows another daemon and is left alone.
public enum Retiring {
  /// A window process from before, by its pid and the arrangement it holds.
  struct Holder: Equatable {
    let pid: pid_t
    let stem: String
  }

  /// What asking came to: the arrangements whose processes quit, for this launch to open, and the
  /// processes still running.
  public struct Outcome: Equatable {
    public var quit: [String] = []
    public var stayed: [pid_t] = []

    public init(quit: [String] = [], stayed: [pid_t] = []) {
      self.quit = quit
      self.stayed = stayed
    }
  }

  /// How long a process asked to quit is given before it is left running.
  private static let patience: TimeInterval = 5

  /// Asks every window process from before under `home` to quit, and waits for them.
  ///
  /// Before the app runs, on the main thread, because what it finds decides which windows this
  /// launch opens: none of this app's windows exist yet to be held up.
  @MainActor
  public static func olderWindowProcesses(home: URL?) -> Outcome {
    guard let home else { return Outcome() }
    let legacy = home.appendingPathComponent("state/windows", isDirectory: true)
    let holders = sameBundle(
      claims(in: legacy), own: ProcessInfo.processInfo.processIdentifier,
      bundle: Bundle.main.bundleURL,
      bundleOf: { NSRunningApplication(processIdentifier: $0)?.bundleURL })
    guard !holders.isEmpty else { return Outcome() }
    for pid in Set(holders.map(\.pid)) {
      NSRunningApplication(processIdentifier: pid)?.terminate()
    }
    let deadline = Date().addingTimeInterval(patience)
    var running = Set(holders.map(\.pid))
    while !running.isEmpty, Date() < deadline {
      running = running.filter(isAlive)
      if !running.isEmpty { usleep(50_000) }
    }
    return outcome(of: holders, stillRunning: running)
  }

  /// The arrangements in a directory with a live claim on them, and the pid holding each.
  static func claims(in directory: URL) -> [Holder] {
    Arrangements.releaseDeadClaims(in: directory)
    let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
    return names.filter { $0.hasSuffix(".held") }.sorted().compactMap { name in
      let claim = directory.appendingPathComponent(name)
      guard let digits = try? String(contentsOf: claim, encoding: .utf8),
        let pid = pid_t(digits.trimmingCharacters(in: .whitespacesAndNewlines))
      else { return nil }
      return Holder(pid: pid, stem: String(name.dropLast(".held".count)))
    }
  }

  /// The holders that are another process of this bundle.
  ///
  /// The paths are compared resolved, because the same app reached through a symlink - Homebrew's
  /// link into /Applications, say - is still the same app.
  static func sameBundle(
    _ holders: [Holder], own: pid_t, bundle: URL?, bundleOf: (pid_t) -> URL?
  ) -> [Holder] {
    guard let bundle = bundle?.resolvingSymlinksInPath().standardizedFileURL else { return [] }
    return holders.filter { holder in
      holder.pid != own
        && bundleOf(holder.pid)?.resolvingSymlinksInPath().standardizedFileURL == bundle
    }
  }

  static func outcome(of holders: [Holder], stillRunning: Set<pid_t>) -> Outcome {
    Outcome(
      quit: holders.filter { !stillRunning.contains($0.pid) }.map(\.stem),
      stayed: stillRunning.sorted())
  }

  /// The arrangements of the windows whose processes quit, in this install's own directory, once
  /// adopted there: a window from before reopens here, even one its process marked closed on the
  /// way out, as every quit used to.
  public static func arrangements(_ outcome: Outcome, in state: URL?) -> [String] {
    guard let windows = Arrangements.directory(state: state) else { return [] }
    return outcome.quit
      .map { windows.appendingPathComponent("\($0).toml").path }
      .filter { FileManager.default.fileExists(atPath: $0) }
  }

  private static func isAlive(_ pid: pid_t) -> Bool {
    kill(pid, 0) == 0 || errno == EPERM
  }
}
