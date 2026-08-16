import Testing

@testable import MusterMac

// Where the file that decides what a Muster pane runs gets written. Every wrong answer here is
// a daemon reading somebody else's config: too far left and it is the user's own herdr file
// again, too far right and it is a path in the filesystem root that nothing can write.

@Test func theDaemonConfigSitsWithTheOtherDerivedFiles() {
  // Beside libghostty.conf rather than beside config.toml, and the distinction is the whole
  // filing rule: config.toml is written by a person, and everything under state/ is written by
  // Muster and rewritten on the next launch.
  #expect(daemonConfigPath(environment: ["HOME": "/home/a"]) == "/home/a/.muster/state/herdr.toml")
}

@Test func musterHomeMovesTheDaemonConfig() {
  // Which is how a test gets a daemon of its own to configure, rather than rewriting the file
  // the developer's own Muster is running from.
  #expect(
    daemonConfigPath(environment: ["MUSTER_HOME": "/scratch", "HOME": "/home/a"])
      == "/scratch/state/herdr.toml")
}

@Test func xdgDoesNotMoveTheDaemonConfig() {
  // The one most worth pinning, because XDG_CONFIG_HOME genuinely does decide where herdr
  // looks for its own config and where it puts its socket. It does not decide this: the file
  // is Muster's, and it is named to the daemon outright rather than found by herdr's rules.
  #expect(
    daemonConfigPath(environment: ["XDG_CONFIG_HOME": "/xdg", "HOME": "/home/a"])
      == "/home/a/.muster/state/herdr.toml")
}

@Test func nowhereToWriteStillAnswers() {
  // Unlike the state path, where nothing means remember nothing. A daemon handed no file falls
  // back to reading the user's own herdr config, which is the behaviour this whole arrangement
  // exists to end - so somewhere writable beats refusing, even somewhere temporary.
  #expect(daemonConfigPath(environment: [:]).hasSuffix("muster-herdr.toml"))
}
