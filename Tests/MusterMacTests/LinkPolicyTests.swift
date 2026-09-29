import Foundation
import MusterRenderer
import Testing

@testable import MusterMac

// What a cmd-clicked link does. Each rule is here because the link is text a program printed:
// it can point at a file that runs code, at another application, or - in a devenv pane - at a
// path on a machine this Mac is not.

@Suite("a cmd-clicked link")
struct LinkPolicyTests {
  private func decide(_ url: String, _ kind: OpenedLink.Kind = .text, here: Bool = true)
    -> LinkPolicy.Decision
  {
    LinkPolicy.decide(OpenedLink(kind: kind, url: url), onThisMachine: here)
  }

  @Test("a web link opens, from a pane on this Mac or on a devenv")
  func aWebLinkOpensFromAnywhere() {
    let url = URL(string: "https://example.com/pr/42")!
    #expect(decide("https://example.com/pr/42") == .open(url))
    #expect(decide("https://example.com/pr/42", .hyperlink, here: false) == .open(url))
    #expect(decide("mailto:someone@example.com") == .open(URL(string: "mailto:someone@example.com")!))
  }

  @Test("a web link with no host, or a mail link with no address, is refused")
  func aMalformedLinkIsRefused() {
    #expect(decide("https:relative") == .refuse("the web link has no host"))
    #expect(decide("mailto:") == .refuse("the mail link has no address"))
    #expect(decide("") == .refuse("the link was empty"))
  }

  @Test("a link that hides characters is refused, whatever it points at")
  func hiddenCharactersAreRefused() {
    // A right-to-left override makes the tail of a link read backwards on screen.
    #expect(
      decide("https://example.com/\u{202E}gpj.exe")
        == .refuse("the link holds invisible or line-breaking characters"))
  }

  @Test("another app's scheme opens when it was on the screen, and asks when it was hidden")
  func aCustomSchemeAsksOnlyWhenItWasHidden() {
    let url = URL(string: "vscode://file/tmp/x")!
    #expect(decide("vscode://file/tmp/x") == .open(url))
    #expect(decide("vscode://file/tmp/x", .hyperlink) == .confirm(url))
  }

  @Test("a path opens from a pane on this Mac, and never from a devenv pane")
  func aPathIsOnlyThisMachines() throws {
    let file = FileManager.default.temporaryDirectory.appending(path: "muster-link-\(UUID()).txt")
    try "notes".write(to: file, atomically: true, encoding: .utf8)
    defer { try? FileManager.default.removeItem(at: file) }

    let resolved = file.standardizedFileURL.resolvingSymlinksInPath()
    #expect(decide(file.path) == .open(resolved))
    #expect(decide(file.absoluteString, .hyperlink) == .open(resolved))

    // The same path from a devenv pane names a file over there. Opening it here would open
    // whatever this Mac has at that path, or nothing.
    let elsewhere = LinkPolicy.Decision.refuse(
      "the path is on the machine the pane runs on, not on this Mac")
    #expect(decide(file.path, here: false) == elsewhere)
    #expect(decide(file.absoluteString, .hyperlink, here: false) == elsewhere)
  }

  @Test("a file that would run code when opened is refused")
  func anExecutableIsRefused() throws {
    let script = FileManager.default.temporaryDirectory.appending(path: "muster-link-\(UUID())")
    try "#!/bin/sh\necho hi\n".write(to: script, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
    defer { try? FileManager.default.removeItem(at: script) }

    #expect(decide(script.path) == .refuse("opening the file could run code"))

    // Launch Services runs a `.command` file in a terminal whatever its permissions say.
    let command = FileManager.default.temporaryDirectory.appending(
      path: "muster-link-\(UUID()).command")
    try "echo hi\n".write(to: command, atomically: true, encoding: .utf8)
    defer { try? FileManager.default.removeItem(at: command) }
    #expect(decide(command.path) == .refuse("opening the file could run code"))
  }

  @Test("a hyperlink with no scheme is refused rather than read as a path")
  func aBareHyperlinkIsRefused() {
    #expect(decide("/etc/hosts", .hyperlink) == .refuse("the hyperlink's target has no scheme"))
  }
}
