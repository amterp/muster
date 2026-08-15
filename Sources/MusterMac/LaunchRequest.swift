import Foundation

// What a command line asked this launch to be. Out of the executable target because it is a
// decision, and decisions in an entry point cannot be reached by a test (docs/testing.md).

/// The three things a launch can be.
public enum LaunchRequest: Equatable {
  /// Show whatever the attached daemons hold. A bare `muster`, and the ordinary case.
  case open

  /// Point the window's keyboard at one named pane, and show its tab.
  case pane(String)

  /// The renderer with no daemon behind it, running the user's shell.
  ///
  /// The only thing that separates "the renderer works" from "the daemon works", which is
  /// worth being able to reach - but behind a flag, because it renders and drops every
  /// keystroke and that is not what somebody who double-clicked an app wants.
  case rendererCheck

  /// A flag Muster does not know.
  ///
  /// Its own case rather than being folded into `open`, because a mistyped flag that silently
  /// opened an ordinary window is a flag nobody finds out did nothing.
  case unknown(String)
}

/// Reads the arguments after the program name.
public func launchRequest(arguments: [String]) -> LaunchRequest {
  guard let first = arguments.first else { return .open }
  if first == "--renderer-check" { return .rendererCheck }
  // Anything else beginning with a dash is a flag, and Muster has one.
  if first.hasPrefix("-") { return .unknown(first) }
  return .pane(first)
}
