import Foundation
import MusterRenderer
import UniformTypeIdentifiers

/// What to do with a link somebody cmd-clicked in a pane.
///
/// A link is text a program printed, so it may point anywhere - including at a file that runs
/// code when opened, or at a custom scheme that hands its contents to another application. The
/// rules are Ghostty's (`UntrustedURL` in its macOS app, at the pinned commit), with one of
/// Muster's own: a pane may be on another machine, where a path names a file there, and opening
/// the same path here would open something else or nothing.
///
/// Pure apart from reading a file's attributes, and separate from the window so that every rule
/// is a case a test can reach.
public enum LinkPolicy {
  public enum Decision: Equatable, Sendable {
    /// Open it with whatever this Mac opens it with.
    case open(URL)
    /// Show it and ask first: a custom scheme hands the link to another application, and an
    /// OSC 8 hyperlink's target is not the text on the screen.
    case confirm(URL)
    /// Leave it, for this reason.
    case refuse(String)
  }

  /// `onThisMachine` is false for a pane on an ssh machine: a web address opens the same from
  /// anywhere, and a path does not.
  public static func decide(_ link: OpenedLink, onThisMachine: Bool) -> Decision {
    let text = link.url
    guard !text.isEmpty else { return .refuse("the link was empty") }
    // Characters that render as nothing, as a line break, or as a reversal of the text around
    // them let a link look like something other than what it opens.
    guard !text.unicodeScalars.contains(where: isUnsafeCharacter) else {
      return .refuse("the link holds invisible or line-breaking characters")
    }

    guard let url = URL(string: text), let scheme = url.scheme?.lowercased(), !scheme.isEmpty
    else {
      // No scheme: a path on the screen, which libghostty has already resolved against the
      // pane's directory. An OSC 8 target must say what it is, so a bare one is malformed.
      guard link.kind == .text else { return .refuse("the hyperlink's target has no scheme") }
      guard onThisMachine else { return elsewhere }
      return file(URL(filePath: NSString(string: text).standardizingPath))
    }

    switch scheme {
    case "http", "https":
      // `https:relative` has a scheme and no host, and consumers resolve it differently.
      guard let host = url.host, !host.isEmpty else { return .refuse("the web link has no host") }
      return .open(url)
    case "mailto":
      guard let parts = URLComponents(url: url, resolvingAgainstBaseURL: false),
        !parts.path.isEmpty
      else { return .refuse("the mail link has no address") }
      return .open(url)
    case "file":
      guard onThisMachine else { return elsewhere }
      return file(url)
    default:
      // Text on the screen is what the reader sees they are opening; a hyperlink's target is not.
      return link.kind == .text ? .open(url) : .confirm(url)
    }
  }

  private static let elsewhere = Decision.refuse(
    "the path is on the machine the pane runs on, not on this Mac")

  /// A file on this Mac, if it is one that opening cannot run.
  static func file(_ url: URL) -> Decision {
    guard url.isFileURL, url.query == nil, url.fragment == nil else {
      return .refuse("the file link is malformed")
    }
    if let host = url.host, !host.isEmpty, host.caseInsensitiveCompare("localhost") != .orderedSame
    {
      return .refuse("the file link names another host")
    }
    // The object the path resolves to, not the spelling: a harmless name can be a symlink to an
    // executable, and `..` can walk anywhere.
    let resolved = url.standardizedFileURL.resolvingSymlinksInPath()
    guard
      let facts = try? resolved.resourceValues(forKeys: [
        .contentTypeKey, .isDirectoryKey, .isExecutableKey, .isRegularFileKey,
      ]),
      facts.isDirectory == true || facts.isRegularFile == true
    else { return .refuse("the path is not a file or folder this Mac can open") }
    if unsafeExtensions.contains(resolved.pathExtension.lowercased())
      || facts.contentType.map({ type in unsafeTypes.contains { type.conforms(to: $0) } }) == true
      || (facts.isDirectory != true && facts.isExecutable == true)
    {
      return .refuse("opening the file could run code")
    }
    return .open(resolved)
  }

  private static func isUnsafeCharacter(_ scalar: Unicode.Scalar) -> Bool {
    switch scalar.value {
    case 0x00...0x1F, 0x7F...0x9F: true  // C0 and C1 controls, line breaks among them
    case 0x061C, 0x200B...0x200F, 0x202A...0x202E, 0x2066...0x2069: true  // direction, zero width
    case 0x2028...0x2029, 0x2060, 0xFEFF: true  // line separators, word joiner, BOM
    default: false
    }
  }

  /// Extensions Launch Services runs or installs, whatever their executable bit says.
  private static let unsafeExtensions: Set<String> = [
    "action", "app", "applescript", "class", "command", "desktop", "inetloc", "jar",
    "mobileconfig", "mpkg", "pkg", "scpt", "terminal", "tool", "url", "webloc", "workflow",
  ]

  /// Types that cover files with a missing or misleading extension.
  private static let unsafeTypes: [UTType] = [.application, .executable, .script]
}
