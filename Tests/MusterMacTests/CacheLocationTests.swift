import Testing

@testable import MusterMac

// Where a downloaded herdr is kept. Only one thing fetches anything - the daemon for a machine
// Muster is attaching over ssh - so the cost of a wrong answer here is 18 MB across the wire on
// every launch rather than once per platform.

@Test func theCacheSitsUnderMustersOwnHome() {
  #expect(cachePath(environment: ["HOME": "/home/a"]) == "/home/a/.muster/cache")
}

@Test func musterHomeMovesTheCache() {
  // Which is what keeps a test that points at a scratch home from writing into the cache the
  // developer's own Muster fetched into.
  #expect(
    cachePath(environment: ["MUSTER_HOME": "/scratch", "HOME": "/home/a"]) == "/scratch/cache")
}

@Test func xdgDoesNotMoveTheCache() {
  // XDG_CACHE_HOME is the conventional answer to this question and is deliberately not the
  // answer here: one directory holds everything Muster owns, and XDG decides where *herdr*
  // keeps things rather than where Muster does.
  #expect(
    cachePath(environment: ["XDG_CACHE_HOME": "/xdg", "HOME": "/home/a"])
      == "/home/a/.muster/cache")
}

@Test func nowhereToKeepAnythingIsAnAnswer() {
  // Unlike the daemon's config, where somewhere temporary beats refusing. A cache that cannot
  // be found costs a download per launch, and a temporary directory that looks like a cache and
  // is emptied by the OS would cost the same while implying otherwise.
  #expect(cachePath(environment: [:]) == nil)
}
