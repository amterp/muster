import Testing

@testable import MusterMac

// Where Muster looks for the file a person wrote. Every wrong answer here is a window with none
// of somebody's daemons in it, which is indistinguishable from every daemon being down - so the
// cost of getting this wrong is a bug report about the wrong subsystem.

/// A filesystem holding exactly these paths, so a case says what is on disk rather than depending
/// on what the developer running it happens to have.
private func holding(_ paths: String...) -> (String) -> Bool {
  let present = Set(paths)
  return { present.contains($0) }
}

@Test func theConfigFileSitsInMustersOwnHome() {
  #expect(
    configPath(
      environment: ["HOME": "/home/a"],
      exists: holding("/home/a/.muster/config.toml")
    ) == "/home/a/.muster/config.toml")
}

@Test func musterHomeMovesTheConfigFile() {
  // Which is how a test or a bug report gets a whole tree of its own, rather than moving each
  // file separately and forgetting one.
  #expect(
    configPath(
      environment: ["MUSTER_HOME": "/scratch", "HOME": "/home/a"],
      exists: holding("/scratch/config.toml")
    ) == "/scratch/config.toml")
}

@Test func xdgDoesNotMoveTheConfigFile() {
  // Muster's own files stopped being XDG-shaped when they moved into one home. XDG_CONFIG_HOME
  // still means something here - it decides where herdr listens - so a reader could reasonably
  // expect it to move this too, which is why the expectation is pinned rather than assumed.
  #expect(
    configPath(
      environment: ["XDG_CONFIG_HOME": "/xdg", "HOME": "/home/a"],
      exists: holding("/xdg/muster/config.toml")
    ) == nil)
}

@Test func anExplicitConfigPathWins() {
  #expect(
    configPath(
      environment: ["MUSTER_CONFIG": "/tmp/one.toml", "HOME": "/home/a"],
      exists: holding("/tmp/one.toml", "/home/a/.muster/config.toml")
    ) == "/tmp/one.toml")
}

@Test func anExplicitConfigPathIsTakenAtItsWord() {
  // Not checked for existence, unlike the default. Somebody who named a file meant that file,
  // and a silent fallback to the ordinary one would be the confusing answer - the core says a
  // named path was unreadable, which is the line worth having.
  #expect(
    configPath(environment: ["MUSTER_CONFIG": "/tmp/gone.toml"], exists: holding())
      == "/tmp/gone.toml")
}

@Test func noFileIsTheOrdinaryCase() {
  #expect(configPath(environment: ["HOME": "/home/a"], exists: holding()) == nil)
}

@Test func nowhereToLookIsAnAnswer() {
  // Rather than a path built from an empty base, which would name something in the filesystem
  // root and read whatever happened to be there.
  #expect(configPath(environment: [:], exists: holding("/config.toml")) == nil)
}

// The file left behind by the move into ~/.muster. Transitional, and these go with it.

@Test func aConfigAtTheOldPathIsReported() {
  #expect(
    strandedConfigPath(
      environment: ["HOME": "/home/a"],
      exists: holding("/home/a/.config/muster/config.toml")
    ) == "/home/a/.config/muster/config.toml")
}

@Test func theOldPathFollowedXdgSoLookingForItDoesToo() {
  #expect(
    strandedConfigPath(
      environment: ["XDG_CONFIG_HOME": "/xdg", "HOME": "/home/a"],
      exists: holding("/xdg/muster/config.toml")
    ) == "/xdg/muster/config.toml")
}

@Test func aFileAtTheNewPathMeansThereIsNothingToSay() {
  // Two files is a choice somebody made, not a move they missed.
  #expect(
    strandedConfigPath(
      environment: ["HOME": "/home/a"],
      exists: holding("/home/a/.muster/config.toml", "/home/a/.config/muster/config.toml")
    ) == nil)
}

@Test func nothingAtEitherPathSaysNothing() {
  #expect(strandedConfigPath(environment: ["HOME": "/home/a"], exists: holding()) == nil)
}
