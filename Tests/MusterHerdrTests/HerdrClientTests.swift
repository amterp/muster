import Testing

@testable import MusterHerdr

// Where a daemon listens is reimplemented from herdr's own rules (src/session.rs,
// src/config/io.rs), which makes it exactly the kind of thing that drifts silently: get it
// wrong and Muster does not crash, it quietly falls back to guessed encodings and arrow
// keys stop working in pagers.

@Test("an explicit socket path beats everything else")
func explicitPathWins() {
  #expect(
    HerdrClient.discoverSocketPath(environment: [
      "HERDR_SOCKET_PATH": "/tmp/explicit.sock",
      "HERDR_SESSION": "work",
      "XDG_CONFIG_HOME": "/xdg",
      "HOME": "/home/a",
    ]) == "/tmp/explicit.sock")
}

@Test("a named session gets its own socket, and the default one does not")
func namedSessions() {
  #expect(
    HerdrClient.discoverSocketPath(environment: ["HOME": "/home/a", "HERDR_SESSION": "work"])
      == "/home/a/.config/herdr/sessions/work/herdr.sock")

  // "default" is spelled by absence rather than by name, so it must not produce a
  // sessions/default directory that no daemon listens in.
  #expect(
    HerdrClient.discoverSocketPath(environment: ["HOME": "/home/a", "HERDR_SESSION": "default"])
      == "/home/a/.config/herdr/herdr.sock")
  #expect(
    HerdrClient.discoverSocketPath(environment: ["HOME": "/home/a", "HERDR_SESSION": ""])
      == "/home/a/.config/herdr/herdr.sock")
}

@Test("XDG_CONFIG_HOME moves the base directory")
func xdgOverridesHome() {
  // The isolated daemon the contract tier spawns works exactly this way, so getting it
  // wrong would make that check silently test nothing.
  #expect(
    HerdrClient.discoverSocketPath(environment: [
      "HOME": "/home/a", "XDG_CONFIG_HOME": "/scratch/config",
    ]) == "/scratch/config/herdr/herdr.sock")
}

@Test("nowhere to look is a nil rather than a guess")
func noHomeNoSocket() {
  // Returning a plausible-looking path would turn "no daemon here" into a connection
  // error somewhere further away from the cause.
  #expect(HerdrClient.discoverSocketPath(environment: [:]) == nil)
  #expect(HerdrClient.discoverSocketPath(environment: ["HOME": ""]) == nil)
}

@Test("an empty explicit path is ignored rather than obeyed")
func emptyExplicitPathFallsThrough() {
  #expect(
    HerdrClient.discoverSocketPath(environment: ["HERDR_SOCKET_PATH": "", "HOME": "/home/a"])
      == "/home/a/.config/herdr/herdr.sock")
}
