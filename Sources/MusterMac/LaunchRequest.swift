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
  let arguments = withoutHome(arguments)
  guard let first = arguments.first else { return .open }
  if first == "--renderer-check" { return .rendererCheck }
  // Anything else beginning with a dash is a flag, and Muster has two.
  if first.hasPrefix("-") { return .unknown(first) }
  return .pane(first)
}

/// Where a launch was told to keep Muster's own files, if it was told.
///
/// An argument rather than only `$MUSTER_HOME`, because of how a second window gets made:
/// `muster window new` starts the app through `open`, which goes via LaunchServices and does
/// not hand over the caller's environment. So a window opened from a pane would land in the
/// default home whatever the pane was told, and a test driving this would land in the
/// developer's own. Passed on the command line, it survives.
///
/// The environment still wins where it is set on this process directly, because that is the
/// case somebody set up deliberately - see `applicationDidFinishLaunching`, which is the one
/// place the two are reconciled.
public func launchHome(arguments: [String]) -> String? {
  guard let at = arguments.firstIndex(of: "--home"), at + 1 < arguments.count else { return nil }
  let home = arguments[at + 1]
  return home.isEmpty ? nil : home
}

/// The arguments with `--home <path>` taken out, so the rest reads as it did before it existed.
private func withoutHome(_ arguments: [String]) -> [String] {
  guard let at = arguments.firstIndex(of: "--home") else { return arguments }
  var rest = arguments
  // The flag and its value, or just the flag when nothing followed it - which then falls
  // through to `.unknown` and is reported, rather than being silently dropped.
  rest.removeSubrange(at..<min(at + 2, rest.count))
  return rest
}
