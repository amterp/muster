import Foundation

/// How this app upgrades itself, worked out once at launch and handed to the core
/// (`Startup.upgrade_command`), which runs it in a tab on this machine's daemon when somebody asks.
///
/// The answer is Homebrew's, and only for the Muster Homebrew installed: Muster checks for nothing
/// on its own and reaches the network only through `brew`, when asked. Any other build is refused
/// with a sentence naming what was looked for, rather than a guess at a path.
///
/// The line runs in a pane because the cask's `uninstall quit:` quits this app partway through, and
/// the pane is the daemon's. Homebrew waits for the quit before replacing the bundle and, since
/// 2026, reopens an app it quit; the `open` after it is for when it did not - an older Homebrew, or
/// an upgrade that failed after the quit - and otherwise only brings the app forward.
public enum Upgrading {
  public enum Plan: Equatable {
    /// The shell line that upgrades this app.
    case run(String)
    /// Why nothing here can, as a sentence for whoever asked.
    case refused(String)
  }

  /// Where Homebrew puts `brew`: Apple Silicon's prefix first, then Intel's.
  public static let brews = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]

  /// The upgrade for the app at `bundle`, from what the file system says about Homebrew.
  ///
  /// The checks are passed in, so a test can stand up a Caskroom and a `brew` that are not there.
  static func plan(
    bundle: URL?, brews: [String] = brews,
    isExecutable: (String) -> Bool, isDirectory: (String) -> Bool, read: (String) -> Data?
  ) -> Plan {
    guard let bundle, bundle.pathExtension == "app" else {
      let path = bundle?.path ?? "an unknown place"
      return .refused(
        "Muster was not upgraded: this one runs from \(path), which is not an app bundle, so no "
          + "package manager installed it. A build from a checkout is upgraded by pulling and "
          + "building it again.")
    }
    let running = resolved(bundle)
    guard let brew = brews.first(where: isExecutable) else {
      return .refused(
        "Muster was not upgraded: there is no Homebrew here (no brew at "
          + "\(brews.joined(separator: " or "))), and Homebrew is the only way Muster knows to "
          + "upgrade itself. A Muster downloaded from GitHub is upgraded by downloading the newer "
          + "release.")
    }
    let prefix = URL(fileURLWithPath: brew).deletingLastPathComponent().deletingLastPathComponent()
    let caskroom = prefix.appendingPathComponent("Caskroom/muster").path
    guard isDirectory(caskroom) else {
      return .refused(
        "Muster was not upgraded: Homebrew at \(brew) has no muster cask (\(caskroom) is not "
          + "there), so it did not install the Muster at \(running) and cannot upgrade it. "
          + "Upgrade it the way it was installed.")
    }
    let metadata = caskroom + "/.metadata"
    let installed = resolved(
      URL(fileURLWithPath: appdir(read(metadata + "/config.json")))
        .appendingPathComponent("Muster.app"))
    guard installed == running else {
      return .refused(
        "Muster was not upgraded: Homebrew's Muster is \(installed), and this one runs from "
          + "\(running), so upgrading Homebrew's would leave this one as it is. Upgrade from the "
          + "Muster at \(installed).")
    }
    let cask = tap(read(metadata + "/INSTALL_RECEIPT.json")).map { "\($0)/muster" } ?? "muster"
    guard let quotedBrew = quoted(brew), let quotedCask = quoted(cask), let app = quoted(running)
    else {
      return .refused(
        "Muster was not upgraded: \(brew) or \(running) holds a quote or a backslash, which "
          + "this cannot hand to every shell intact. Run `brew upgrade --cask muster` in a pane "
          + "yourself.")
    }
    return .run(
      "\(quotedBrew) upgrade --cask \(quotedCask); /usr/bin/env -i /usr/bin/open -a \(app)")
  }

  /// The upgrade for this process, from the real file system.
  public static func here() -> Plan {
    let files = FileManager.default
    return plan(
      bundle: Bundle.main.bundleURL,
      isExecutable: { files.isExecutableFile(atPath: $0) },
      isDirectory: { path in
        var directory: ObjCBool = false
        return files.fileExists(atPath: path, isDirectory: &directory) && directory.boolValue
      },
      read: { files.contents(atPath: $0) })
  }

  /// Where the cask put the app: what `--appdir` said, then `HOMEBREW_CASK_OPTS`, then Homebrew's
  /// default, as the cask's own `config.json` records them.
  static func appdir(_ config: Data?) -> String {
    let recorded = config.flatMap { try? JSONSerialization.jsonObject(with: $0) } as? [String: Any]
    for layer in ["explicit", "env", "default"] {
      if let appdir = (recorded?[layer] as? [String: Any])?["appdir"] as? String, !appdir.isEmpty {
        return appdir
      }
    }
    return "/Applications"
  }

  /// The tap the cask came from, as its install receipt records it: `amterp/tap`.
  static func tap(_ receipt: Data?) -> String? {
    let recorded = receipt.flatMap { try? JSONSerialization.jsonObject(with: $0) } as? [String: Any]
    let tap = (recorded?["source"] as? [String: Any])?["tap"] as? String
    return tap?.isEmpty == false ? tap : nil
  }

  /// Single-quoted for sh, bash, zsh and fish alike, or nil for text that none of them would read
  /// back unchanged: inside single quotes fish still treats `\` as an escape, and a `'` ends them.
  static func quoted(_ text: String) -> String? {
    text.contains("'") || text.contains("\\") ? nil : "'\(text)'"
  }

  /// The path with symlinks followed, since the same app reached through a link is still the
  /// same app.
  private static func resolved(_ url: URL) -> String {
    url.resolvingSymlinksInPath().standardizedFileURL.path
  }
}
