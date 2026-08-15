import Testing

@testable import MusterMac

// Where a window's arrangement is remembered. Small enough to look obvious and worth pinning
// anyway: every wrong answer here is a window that silently forgets its layout, or one that
// writes into a directory somebody else owns.

@Test func theStateFileSitsWhereRegeneratedFilesBelong() {
  // Not beside the config file. That one is written by a person and would be surprising to
  // find rewritten; this one is written by Muster on every change.
  #expect(
    statePath(environment: ["HOME": "/home/a"]) == "/home/a/.local/state/muster/window.toml")
}

@Test func xdgMovesIt() {
  // Which is how every test and every recording gets its own, rather than sharing the
  // developer's real one.
  #expect(
    statePath(environment: ["XDG_STATE_HOME": "/xdg", "HOME": "/home/a"])
      == "/xdg/muster/window.toml")
}

@Test func anExplicitPathWins() {
  #expect(
    statePath(environment: ["MUSTER_STATE": "/tmp/one.toml", "HOME": "/home/a"])
      == "/tmp/one.toml")
}

@Test func anEmptyExplicitPathMeansRememberNothing() {
  // The difference between "look somewhere else" and "do not remember", which a script or a
  // test needs and which an absent variable cannot say.
  #expect(statePath(environment: ["MUSTER_STATE": "", "HOME": "/home/a"]) == nil)
}

@Test func nowhereToWriteIsAnAnswer() {
  // A window that opens fresh every time, which is what it did before any of this existed -
  // rather than a path built from an empty base, which would name something in the
  // filesystem root.
  #expect(statePath(environment: [:]) == nil)
}
