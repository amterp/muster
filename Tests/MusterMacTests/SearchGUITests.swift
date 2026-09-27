import AppKit
import MusterRenderer
import Testing

@testable import MusterMac

// What libghostty answers when Muster's find bar asks it to search.
//
// The claim behind find that no other test can reach. The bar draws counts it never computes:
// they come back from libghostty as runtime actions, on libghostty's threads, and reach a
// surface only through `rendererAction`'s routing by the token the surface was created with. A
// mismatch anywhere on that path - an action renamed at a pin bump, a count that never leaves
// the renderer - is a find bar that searches and counts nothing, and every other test here
// replaces the surface with a recording.
//
// One test with a real surface: a real runtime, a real command behind a pty, and a Metal layer
// on a view. Two runtimes in one process hang, so it is one test rather than several.
//
// **Out of the default gate**, for the reason `--contract` is: a Metal layer wants a logged-in
// session, and the gate is not allowed to need one (docs/testing.md). `./dev --contract` sets
// `MUSTER_GUI_TESTS` and runs it; without that it skips, and an ordinary gate run lists it as
// skipped so that a reader can tell it exists.

/// Set by `./dev --contract`, which is the tier that has a session to draw in.
private let hasGUISession = ProcessInfo.processInfo.environment["MUSTER_GUI_TESTS"] == "1"

@Suite("a real surface's search", .enabled(if: hasGUISession, "needs a logged-in session"))
struct SearchGUITests {
  @MainActor
  @Test("counts its matches, steps through them, and says when it has ended")
  func aSearchIsCountedSteppedAndEnded() async throws {
    let renderer = try Renderer(configPath: NSTemporaryDirectory() + "muster-search.conf")
    Renderer.current = renderer
    defer { Renderer.current = nil }
    let view = NSView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
    // Short-lived on purpose: a command outliving the test is a process left on somebody's
    // machine, and the surface only has to stay alive while this runs.
    //
    // A shell of its own, because libghostty runs a command as `exec -l <command>` under a login
    // shell, so a list of commands is the first one replacing the shell and the rest never
    // running. And the needle is assembled rather than written out, because a command that
    // fails puts its own text on the screen, where a search counts it.
    let surface = try renderer.makeSurface(
      in: view,
      command:
        "/bin/sh -c \"printf '%sdle\\n' nee nee; echo hay; printf '%sdle\\n' nee; sleep 10\"")
    surface.setSize(width: 800, height: 600)
    var reports: [SearchReport] = []
    surface.onSearch = { reports.append($0) }

    // Searched for afresh until it is counted, because the text reaches the surface through its
    // pty some time after the surface exists. A search of an empty screen counts nothing, and
    // asking again for the same needle is not a new search - libghostty answers nothing more.
    try await answered("the three needles to be counted") {
      surface.search(nil)
      surface.search("needle")
      return await polled(within: .milliseconds(250)) { reports.contains(.total(3)) }
    }

    // Stepping selects a match. Which one first is libghostty's business; that a second step
    // moves it is what the bar's next and previous rely on.
    reports.removeAll()
    surface.navigateSearch(next: true)
    try await answered("a step to select a match") { selected(reports) != nil }
    let first = selected(reports)
    reports.removeAll()
    surface.navigateSearch(next: true)
    try await answered("a second step to select another") { selected(reports) != nil }
    #expect(selected(reports) != first)

    // Ending clears both counts, which is what takes a stale count out of the bar.
    reports.removeAll()
    surface.search(nil)
    try await answered("the search to end") {
      reports.contains(.total(nil)) && reports.contains(.selected(nil))
    }
  }
}

/// The last match libghostty reported selected, if it has reported one.
private func selected(_ reports: [SearchReport]) -> Int? {
  for report in reports.reversed() {
    if case .selected(let index) = report { return index }
  }
  return nil
}

/// Waits for something libghostty will do on its own, or records what it was waiting for.
///
/// A bounded poll that gives up the main actor each turn, because every answer here arrives as
/// a hop to it: libghostty's wakeup schedules the tick that drains the surface's mailbox, and
/// its action callback schedules the report.
@MainActor
private func answered(
  _ what: String, within: Duration = .seconds(10), _ ready: @MainActor () async -> Bool
) async throws {
  if await polled(within: within, ready) { return }
  Issue.record("timed out after \(within) waiting for \(what)")
  throw CancellationError()
}

@MainActor
private func polled(within: Duration, _ ready: @MainActor () async -> Bool) async -> Bool {
  let deadline = ContinuousClock.now + within
  while ContinuousClock.now < deadline {
    if await ready() { return true }
    try? await Task.sleep(for: .milliseconds(5))
  }
  return await ready()
}
