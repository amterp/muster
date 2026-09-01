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
  /// - Parameter herdrSocketPath: the daemon to ask for this pane's frames, spelled the way
  ///   the machine that will open it spells it - so for a remote pane this is a path on the
  ///   far machine rather than the near end of the tunnel. Absent means the bridge finds one
  ///   for itself, which reaches the right daemon only by luck: Muster's listens on a herdr
  ///   session of its own.
  /// - Parameter herdrBinaryPath: the daemon binary to run, on this machine. Only for a local
  ///   pane: a remote bridge runs its CLI on the far machine, where a path from here names
  ///   nothing, and it prefers the herdr Muster installed over there. Absent means the bridge
  ///   looks for one beside itself and then on PATH, which is right for a bridge somebody ran
  ///   by hand and wrong for every shipped bundle (kan a_2Hnh3g0Y5).
  /// - Parameter reattaching: whether this window has had a bridge for this pane before, which
  ///   is the one case where taking the terminal over is right. Only one client may hold a
  ///   herdr terminal, and a client whose transport died goes on holding one - measured at 53
  ///   minutes after the ssh carrying it was gone - so without this every pane a network change
  ///   touched stays locked until somebody kills the far-side process by hand. A first bridge
  ///   never takes over: the terminal it would take could be one another window is showing.
  public static func bridge(
    executable: String, paneID: String, controlSocketPath: String?,
    herdrSocketPath: String? = nil, herdrBinaryPath: String? = nil,
    sshHost: String? = nil, sshControlPath: String? = nil,
    reattaching: Bool = false
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
    let elsewhere: (host: String, control: String)? =
      if let sshHost, let sshControlPath, !sshHost.isEmpty, !sshControlPath.isEmpty {
        (sshHost, sshControlPath)
      } else {
        nil
      }
    // A path on this machine, so only for a pane on this machine - and read off the same
    // answer the ssh arguments are, or half a target would build a local command with the
    // daemon left out of it.
    if let herdrBinaryPath, !herdrBinaryPath.isEmpty, elsewhere == nil {
      arguments += ["--herdr-binary", herdrBinaryPath]
    }
    if let elsewhere {
      arguments += ["--via-ssh", elsewhere.host, "--ssh-control", elsewhere.control]
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
  /// carried spaces since forever, and the ssh destination now comes out of a config file
  /// somebody typed - and the failure is a pane that renders nothing for a reason no log
  /// line would name.
  private static func quoted(_ argument: String) -> String {
    "'" + argument.replacingOccurrences(of: "'", with: "'\\''") + "'"
  }
}
