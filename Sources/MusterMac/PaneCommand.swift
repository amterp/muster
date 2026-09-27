import Foundation

/// What a surface should run to show a pane.
///
/// libghostty can only be fed by the command its surface spawns, so this string is the
/// entire interface between the app and a pane's stream - and it is assembled by hand from
/// parts that each have to be right. A wrong bridge path or a dropped socket argument does not
/// fail loudly; it produces a window that renders nothing, which is the symptom that has cost
/// the most time here.
///
/// Its own function, out of the executable, for that reason.
public enum PaneCommand {
  /// The command for a named pane.
  ///
  /// - Parameter paneID: what Muster calls the pane, which is also what its daemon knows it by.
  /// - Parameter daemonSocket: where the bridge dials to draw the pane - the daemon's own
  ///   socket, or for a daemon on another machine the local end of the ssh forward to it.
  /// - Parameter linkSocketPath: where the bridge reports that it attached, that it is
  ///   painting, and why it exits. A bridge the window cannot hear from is one whose death
  ///   nobody notices, so the caller does not build this until there is one.
  /// - Parameter remote: whether that daemon is on another machine, where the bridge asks for
  ///   a window of unacknowledged output sized for the link.
  /// - Parameter reattaching: whether this window has had a bridge for this pane before, which
  ///   is the one case where displacing a bridge already drawing the pane is right: the one
  ///   there is most often this window's own, on the far side of a connection that died. A
  ///   first bridge never takes over, because the pane could be one another window is showing.
  public static func bridge(
    executable: String, paneID: String, daemonSocket: String, linkSocketPath: String,
    remote: Bool = false, reattaching: Bool = false
  ) -> String {
    let bridge = URL(fileURLWithPath: executable)
      .deletingLastPathComponent()
      .appendingPathComponent("muster-bridge")
      .path

    var arguments = [
      bridge, paneID, "--daemon-socket", daemonSocket, "--app-socket", linkSocketPath,
    ]
    if remote {
      arguments += ["--remote"]
    }
    if reattaching {
      arguments += ["--takeover"]
    }
    return arguments.map(quoted).joined(separator: " ")
  }

  /// One argument, safe to hand to a command line that will be split on spaces.
  ///
  /// Everything here reaches libghostty as a single string and is word-split on the way to a
  /// process, so an unquoted value with a space in it becomes two arguments. Paths have
  /// carried spaces since forever, and the failure is a pane that renders nothing for a reason
  /// no log line would name.
  private static func quoted(_ argument: String) -> String {
    "'" + argument.replacingOccurrences(of: "'", with: "'\\''") + "'"
  }
}
