import Foundation

// How many files this process may hold open. An OS question, and therefore the shell's: the
// core opens the sockets, and this decides how many it is allowed to. Ports pick their own
// answer here and nothing below changes.

/// Raises this process's file-descriptor ceiling before anything opens a socket.
///
/// The ceiling is one nobody working on Muster meets by accident. A shell reports a soft
/// `RLIMIT_NOFILE` in the millions; a process launched from the Dock inherits launchd's,
/// which defaults to 256 soft against an effectively unlimited hard - so this only bites once
/// there is a bundle to double-click, and it bites as a pane that silently never opens.
///
/// A full window is closer to that 256 than it looks. Each agent watcher costs two
/// descriptors rather than one, because herdr's `watch()` clones the stream so the watcher
/// can shut itself down (`docs/observations/herdr-0.8.0.md`, section 11) - so fifteen panes on
/// each of two daemons is 60 before the structure subscription per daemon, the control socket
/// per visible pane, and the bridge behind each.
public enum DescriptorLimit {
  /// What Muster asks for.
  ///
  /// About ten times the busiest window anybody can fill, which leaves room for the panes,
  /// the bridges, the ssh masters and whatever a future arc opens without this number needing
  /// to be revisited. Not unlimited: macOS caps a process at `kern.maxfilesperproc` anyway,
  /// and a request for everything is a request nobody can reason about afterwards.
  public static let wanted: UInt64 = 4096

  /// Below this, a full window is at risk rather than merely tight.
  ///
  /// Two daemons of fifteen panes each cost roughly 190 descriptors by the arithmetic above,
  /// so this is that with room for the process itself. Its job is to make an unraisable
  /// ceiling say so at launch instead of surfacing as one blank pane in an hour's time.
  public static let needed: UInt64 = 512

  /// What was asked for, and what the kernel granted.
  public struct Outcome: Sendable {
    public let before: UInt64
    public let after: UInt64
    public let hard: UInt64
    /// What was asked for, or nil when the process already had enough.
    public let asked: UInt64?
    /// Why the kernel said no, when it did.
    public let refusal: String?

    /// Whether what we ended up with covers a full window.
    public var sufficient: Bool { after >= needed }
  }

  /// What a process holding `soft` of `hard` should ask for.
  ///
  /// Nil means there is nothing to ask: either the soft limit is already enough, or it is
  /// already at the hard one and asking again would be a syscall that changes nothing. Pure,
  /// and separate from the syscall, because it is the half that can be wrong.
  public static func request(soft: UInt64, hard: UInt64, wanted: UInt64 = wanted) -> UInt64? {
    guard soft < wanted else { return nil }
    let ask = min(wanted, hard)
    return ask > soft ? ask : nil
  }

  /// Reads the current limit, raises it if that would help, and says what happened.
  ///
  /// Never fatal. A window that could not raise its ceiling still runs and still shows panes;
  /// what it must not do is meet the ceiling silently, which is what [`report`] is for.
  public static func raise() -> Outcome {
    var limits = rlimit()
    guard getrlimit(RLIMIT_NOFILE, &limits) == 0 else {
      return Outcome(
        before: 0, after: 0, hard: 0, asked: nil,
        refusal: "the current limit could not be read: \(errnoText())")
    }
    let before = UInt64(limits.rlim_cur)
    let hard = UInt64(limits.rlim_max)
    guard let ask = request(soft: before, hard: hard) else {
      return Outcome(before: before, after: before, hard: hard, asked: nil, refusal: nil)
    }

    limits.rlim_cur = rlim_t(ask)
    guard setrlimit(RLIMIT_NOFILE, &limits) == 0 else {
      return Outcome(
        before: before, after: before, hard: hard, asked: ask, refusal: errnoText())
    }
    return Outcome(before: before, after: ask, hard: hard, asked: ask, refusal: nil)
  }

  /// Puts the outcome on the record, and says what a short one costs.
  ///
  /// Separate from [`raise`] because the two happen at different moments: the limit is raised
  /// before the core opens anything, and the core owns the log file from a moment later.
  public static func report(_ outcome: Outcome) {
    let facts = [
      "before": String(outcome.before),
      "after": String(outcome.after),
      "hard": String(outcome.hard),
      "asked": outcome.asked.map(String.init) ?? "(nothing to ask)",
    ]
    guard outcome.refusal == nil, outcome.sufficient else {
      Core.warn(
        "descriptors.short",
        facts.merging([
          "detail": outcome.refusal ?? "the hard limit is lower than a full window needs",
          "impact": "a window this size opens \(outcome.after) files before panes start "
            + "failing to open, and a pane that fails this way renders nothing rather than "
            + "reporting anything",
          "check": "`launchctl limit maxfiles` and `ulimit -Hn` for the ceiling this process "
            + "inherited; a full window wants at least \(needed)",
        ]) { _, replacement in replacement })
      return
    }
    Core.info("descriptors.raised", facts)
  }

  private static func errnoText() -> String {
    String(cString: strerror(errno)) + " (errno \(errno))"
  }
}
