import Foundation
import Testing

@testable import MusterMac

// What the app hands the core as its way to upgrade itself. Pinned without Homebrew: the file
// system is a table of what exists, so each case is one install somebody could have. A wrong
// answer here is either an upgrade of some other Muster, which quits this one for nothing, or a
// refusal of the Muster Homebrew did install.

@Suite("upgrading through Homebrew")
struct UpgradingTests {
  /// A machine as the checks see it: which brews run, which directories exist, what files say.
  struct Machine {
    var brews: Set<String> = ["/opt/homebrew/bin/brew"]
    var directories: Set<String> = ["/opt/homebrew/Caskroom/muster"]
    var files: [String: String] = [
      "/opt/homebrew/Caskroom/muster/.metadata/config.json":
        #"{"default":{"appdir":"/Applications"},"env":{},"explicit":{}}"#,
      "/opt/homebrew/Caskroom/muster/.metadata/INSTALL_RECEIPT.json":
        #"{"source":{"tap":"amterp/tap","version":"0.13.0"}}"#,
    ]

    func plan(_ bundle: String?) -> Upgrading.Plan {
      Upgrading.plan(
        bundle: bundle.map { URL(fileURLWithPath: $0) },
        isExecutable: { brews.contains($0) },
        isDirectory: { directories.contains($0) },
        read: { files[$0].map { Data($0.utf8) } })
    }
  }

  @Test("the Muster Homebrew installed upgrades its cask from its tap, then opens again")
  func homebrewsMusterUpgrades() {
    #expect(
      Machine().plan("/Applications/Muster.app")
        == .run(
          "'/opt/homebrew/bin/brew' upgrade --cask 'amterp/tap/muster'; "
            + "/usr/bin/env -i /usr/bin/open -a '/Applications/Muster.app'"))
  }

  @Test("an Intel Mac's Homebrew is found where it keeps it")
  func intelHomebrewIsFound() {
    var machine = Machine()
    machine.brews = ["/usr/local/bin/brew"]
    machine.directories = ["/usr/local/Caskroom/muster"]
    #expect(
      machine.plan("/Applications/Muster.app")
        == .run(
          "'/usr/local/bin/brew' upgrade --cask 'muster'; "
            + "/usr/bin/env -i /usr/bin/open -a '/Applications/Muster.app'"))
  }

  @Test("an --appdir given at install moves where Homebrew's Muster is")
  func anExplicitAppdirWins() {
    var machine = Machine()
    machine.files["/opt/homebrew/Caskroom/muster/.metadata/config.json"] =
      #"{"default":{"appdir":"/Applications"},"explicit":{"appdir":"/Users/a/Applications"}}"#
    guard case .run(let line) = machine.plan("/Users/a/Applications/Muster.app") else {
      Issue.record("refused the Muster Homebrew put where it was told to")
      return
    }
    #expect(line.hasSuffix("open -a '/Users/a/Applications/Muster.app'"))
    #expect(refusal(machine.plan("/Applications/Muster.app")).contains("/Users/a/Applications"))
  }

  @Test("a build run from a checkout is not an app, and says so")
  func aBuildIsNotAnApp() {
    let refused = refusal(Machine().plan("/src/muster/.build/debug"))
    #expect(refused.contains("/src/muster/.build/debug"))
    #expect(refused.contains("not an app bundle"))
  }

  @Test("a checkout's bundle beside the cask is not upgraded in its place")
  func aCheckoutsBundleIsNotHomebrews() {
    let refused = refusal(Machine().plan("/src/muster/.build/muster.app"))
    #expect(refused.contains("/Applications/Muster.app"))
    #expect(refused.contains("/src/muster/.build/muster.app"))
  }

  @Test("no Homebrew names both places it looked")
  func noHomebrewNamesWhereItLooked() {
    var machine = Machine()
    machine.brews = []
    let refused = refusal(machine.plan("/Applications/Muster.app"))
    #expect(refused.contains("/opt/homebrew/bin/brew"))
    #expect(refused.contains("/usr/local/bin/brew"))
  }

  @Test("Homebrew with no muster cask did not install this one")
  func noCaskIsNotHomebrews() {
    var machine = Machine()
    machine.directories = []
    let refused = refusal(machine.plan("/Applications/Muster.app"))
    #expect(refused.contains("/opt/homebrew/Caskroom/muster"))
  }

  @Test("a path no shell would read back intact is refused rather than mangled")
  func aQuoteInAPathIsRefused() {
    var machine = Machine()
    machine.files["/opt/homebrew/Caskroom/muster/.metadata/config.json"] =
      #"{"explicit":{"appdir":"/Users/o'neil/Apps"}}"#
    #expect(refusal(machine.plan("/Users/o'neil/Apps/Muster.app")).contains("quote"))
  }

  @Test("an unreadable record falls back to Homebrew's defaults")
  func unreadableRecordsFallBack() {
    #expect(Upgrading.appdir(Data("not json".utf8)) == "/Applications")
    #expect(Upgrading.appdir(nil) == "/Applications")
    #expect(Upgrading.tap(Data(#"{"source":{"tap":""}}"#.utf8)) == nil)
  }

  private func refusal(_ plan: Upgrading.Plan) -> String {
    guard case .refused(let why) = plan else {
      Issue.record("expected a refusal, got \(plan)")
      return ""
    }
    #expect(why.hasPrefix("Muster was not upgraded: "))
    return why
  }
}
