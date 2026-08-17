import Foundation
import Testing

@testable import MusterMac

// How a pane comes to find `muster` on its PATH. Worth pinning because every wrong answer here is
// an agent that cannot drive the window it is drawn in, and the two failure shapes look nothing
// alike: no link at all is a `command not found`, and a link to a binary that has gone is a
// `muster` that exists and refuses to run, which reads as a broken install.

@Test func commandsLiveBesideStateRatherThanInsideIt() {
  #expect(commandsPath(environment: ["HOME": "/home/a"]) == "/home/a/.muster/bin")
}

@Test func musterHomeMovesTheCommandDirectory() {
  #expect(
    commandsPath(environment: ["MUSTER_HOME": "/scratch", "HOME": "/home/a"]) == "/scratch/bin")
}

@Test func anEmptyExplicitDirectoryMeansPutNothingOnAnyPath() {
  #expect(commandsPath(environment: ["MUSTER_COMMANDS": "", "HOME": "/home/a"]) == nil)
}

@Test func nowhereToLookIsARealAnswer() {
  #expect(commandsPath(environment: [:]) == nil)
}

@Test func theLinkPointsAtTheCliThisBuildStaged() throws {
  let scratch = try scratchDirectory()
  defer { try? FileManager.default.removeItem(at: scratch) }
  let staged = try stagedCommand(in: scratch)
  let commands = scratch.appendingPathComponent("bin").path

  let answered = refreshMusterCommand(
    executable: scratch.appendingPathComponent("muster").path, commands: commands)

  #expect(answered == commands)
  let link = URL(fileURLWithPath: commands).appendingPathComponent("muster")
  #expect(
    try FileManager.default.destinationOfSymbolicLink(atPath: link.path) == staged.path,
    """
    the link should point at the staged CLI; a copy would be a stale muster talking to a \
    window it no longer matches
    """)
  #expect(FileManager.default.isExecutableFile(atPath: link.path))
}

@Test func aLinkLeftByAnEarlierBuildIsRepointed() throws {
  let scratch = try scratchDirectory()
  defer { try? FileManager.default.removeItem(at: scratch) }
  let staged = try stagedCommand(in: scratch)
  let commands = scratch.appendingPathComponent("bin")
  try FileManager.default.createDirectory(at: commands, withIntermediateDirectories: true)
  try FileManager.default.createSymbolicLink(
    at: commands.appendingPathComponent("muster"),
    withDestinationURL: scratch.appendingPathComponent("an-older-build/muster-cli"))

  _ = refreshMusterCommand(
    executable: scratch.appendingPathComponent("muster").path, commands: commands.path)

  #expect(
    try FileManager.default.destinationOfSymbolicLink(
      atPath: commands.appendingPathComponent("muster").path) == staged.path,
    """
    an app that moved - a build to a bundle, one version to the next - has to repoint the link, \
    or the command on somebody's PATH is the one from wherever it used to live
    """)
}

@Test func aDanglingLinkIsTakenAwayRatherThanLeftOnThePath() throws {
  let scratch = try scratchDirectory()
  defer { try? FileManager.default.removeItem(at: scratch) }
  let commands = scratch.appendingPathComponent("bin")
  try FileManager.default.createDirectory(at: commands, withIntermediateDirectories: true)
  let link = commands.appendingPathComponent("muster")
  try FileManager.default.createSymbolicLink(
    at: link, withDestinationURL: scratch.appendingPathComponent("gone/muster-cli"))

  // No staged CLI beside this executable, which is what a checkout that never built one looks
  // like.
  let answered = refreshMusterCommand(
    executable: scratch.appendingPathComponent("muster").path, commands: commands.path)

  #expect(answered == nil, "with no command to offer, the core should be told nothing")
  #expect(
    !FileManager.default.fileExists(atPath: link.path),
    """
    a link whose target has gone is worse than no link: `muster` would exist, fail to exec, and \
    look like a broken install rather than an absent one
    """)
}

@Test func somebodyElsesFileInThatDirectoryIsLeftAlone() throws {
  let scratch = try scratchDirectory()
  defer { try? FileManager.default.removeItem(at: scratch) }
  let commands = scratch.appendingPathComponent("bin")
  try FileManager.default.createDirectory(at: commands, withIntermediateDirectories: true)
  let theirs = commands.appendingPathComponent("muster")
  try Data("their own script".utf8).write(to: theirs)

  _ = refreshMusterCommand(
    executable: scratch.appendingPathComponent("muster").path, commands: commands.path)

  #expect(
    try String(data: Data(contentsOf: theirs), encoding: .utf8) == "their own script",
    """
    this directory is one a person may well put on their own PATH, so a launch with nothing to \
    offer must not delete what somebody put there
    """)
}

@Test func noDirectoryToUseIsNotAnError() {
  #expect(refreshMusterCommand(executable: "/nowhere/muster", commands: nil) == nil)
}

private func scratchDirectory() throws -> URL {
  let scratch = URL(fileURLWithPath: NSTemporaryDirectory())
    .appendingPathComponent("muster-commands-\(UUID().uuidString)", isDirectory: true)
  try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
  return scratch
}

/// A stand-in for the CLI `./dev` stages beside the app executable.
private func stagedCommand(in scratch: URL) throws -> URL {
  let staged = scratch.appendingPathComponent("muster-cli")
  try Data("#!/bin/sh\n".utf8).write(to: staged)
  try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: staged.path)
  return staged
}
