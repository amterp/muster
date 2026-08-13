import Foundation

/// What a surface should run to show a pane.
///
/// libghostty can only be fed by the command its surface spawns, so this string is the
/// entire interface between the app and a pane's frame stream - and it is assembled by
/// hand from three parts that each have to be right. A wrong bridge path or a dropped
/// socket argument does not fail loudly; it produces a window that renders and ignores the
/// keyboard, which is the symptom that has cost the most time here.
///
/// Its own function, out of the executable, for that reason.
public enum PaneCommand {
  /// The command for a named pane.
  ///
  /// - Parameter controlSocketPath: where the bridge should dial back to reach the app.
  ///   Absent means the pane renders but cannot be typed into, which is a real state - the
  ///   app may have failed to bind - and not one to paper over.
  public static func bridge(
    executable: String, paneID: String, controlSocketPath: String?
  ) -> String {
    let bridge = URL(fileURLWithPath: executable)
      .deletingLastPathComponent()
      .appendingPathComponent("muster-bridge")
      .path
    guard let controlSocketPath else { return "\(bridge) \(paneID)" }
    return "\(bridge) \(paneID) --control-socket \(controlSocketPath)"
  }
}
