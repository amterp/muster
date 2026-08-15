import Foundation

/// What a surface should run to show a pane.
///
/// libghostty can only be fed by the command its surface spawns, so this string is the
/// entire interface between the app and a pane's frame stream - and it is assembled by
/// hand from parts that each have to be right. A wrong bridge path or a dropped socket
/// argument does not fail loudly; it produces a window that renders and ignores the
/// keyboard, which is the symptom that has cost the most time here.
///
/// Its own function, out of the executable, for that reason.
public enum PaneCommand {
  /// The command for a named pane.
  ///
  /// - Parameter controlSocketPath: where the bridge should dial back to reach the app.
  ///   Absent means the pane renders but cannot be typed into, which is a real state - the
  ///   app may have failed to bind - and not one to paper over.
  /// - Parameter sshHost: the machine this pane lives on, when it is not this one, and
  ///   `sshControlPath` the master the core already opened for that daemon. Both come from
  ///   the view rather than being worked out here: they name a connection the core owns.
  /// - Parameter herdrSocketPath: the daemon to ask for this pane's frames. Absent means the
  ///   bridge finds one for itself, which is right only for a remote pane - its command runs
  ///   on the far machine, where a path from this one names nothing.
  public static func bridge(
    executable: String, paneID: String, controlSocketPath: String?,
    herdrSocketPath: String? = nil,
    sshHost: String? = nil, sshControlPath: String? = nil
  ) -> String {
    let bridge = URL(fileURLWithPath: executable)
      .deletingLastPathComponent()
      .appendingPathComponent("muster-bridge")
      .path

    var arguments = [bridge, paneID]
    if let controlSocketPath {
      arguments += ["--control-socket", controlSocketPath]
    }
    if let herdrSocketPath, !herdrSocketPath.isEmpty {
      arguments += ["--herdr-socket", herdrSocketPath]
    }
    // Half a target would run the wrong machine's terminal, so both or neither. The bridge
    // refuses half as well; this is the side that can still say something useful about it.
    if let sshHost, let sshControlPath, !sshHost.isEmpty, !sshControlPath.isEmpty {
      arguments += ["--via-ssh", sshHost, "--ssh-control", sshControlPath]
    }
    return arguments.map(quoted).joined(separator: " ")
  }

  /// One argument, safe to hand to a command line that will be split on spaces.
  ///
  /// Everything here reaches libghostty as a single string and is word-split on the way to a
  /// process, so an unquoted value with a space in it becomes two arguments. Paths have
  /// carried spaces since forever, and the ssh destination now comes out of a config file
  /// somebody typed - and the failure is a pane that renders nothing for a reason no log
  /// line would name.
  private static func quoted(_ argument: String) -> String {
    "'" + argument.replacingOccurrences(of: "'", with: "'\\''") + "'"
  }
}
