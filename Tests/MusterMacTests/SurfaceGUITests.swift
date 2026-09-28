import AppKit
import MusterRenderer
import Testing

@testable import MusterMac

// What a real libghostty surface does with what Muster hands it.
//
// Search first: the claim behind find that no other test can reach. The bar draws counts it never computes:
// they come back from libghostty as runtime actions, on libghostty's threads, and reach a
// surface only through `rendererAction`'s routing by the token the surface was created with. A
// mismatch anywhere on that path - an action renamed at a pin bump, a count that never leaves
// the renderer - is a find bar that searches and counts nothing, and every other test here
// replaces the surface with a recording.
//
// Then a shift-drag, which a program can ask to be sent rather than selected with (XTSHIFTESCAPE).
// The daemon honors that in its own mouse encoding; the surface has to agree, or a shift-drag
// both selects text and reaches the program. It reads the request from the pane's own bytes,
// which the bridge writes to it unchanged and a replay restates on attach, so no test that
// replaces the surface can say whether it does.
//
// Real surfaces: a real runtime, a real command behind a pty, and a Metal layer on a view. Two
// runtimes in one process hang, so the tests share one and take turns.
//
// **Out of the default gate**, for the reason `--contract` is: a Metal layer wants a logged-in
// session, and the gate is not allowed to need one (docs/testing.md). `./dev --contract` sets
// `MUSTER_GUI_TESTS` and runs it; without that it skips, and an ordinary gate run lists it as
// skipped so that a reader can tell it exists.

/// Set by `./dev --contract`, which is the tier that has a session to draw in.
private let hasGUISession = ProcessInfo.processInfo.environment["MUSTER_GUI_TESTS"] == "1"

@Suite("a real surface", .serialized, .enabled(if: hasGUISession, "needs a logged-in session"))
struct SurfaceGUITests {
  @MainActor
  @Test("counts its matches, steps through them, and says when it has ended")
  func aSearchIsCountedSteppedAndEnded() async throws {
    let renderer = try sharedRenderer()
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

extension SurfaceGUITests {
  @MainActor
  @Test("leaves a shift-drag to a program that asked for it, and selects with it otherwise")
  func aShiftDragIsTheProgramsWhenItAsked() async throws {
    #expect(try await shiftDragSelects(programAsked: false))
    #expect(try !(await shiftDragSelects(programAsked: true)))
  }
}

/// Whether a shift-drag across the first row selects it, over a program reporting the mouse that
/// asked (CSI > 1 s) or declined (CSI > 0 s) to be sent shift-clicks.
@MainActor
private func shiftDragSelects(programAsked: Bool) async throws -> Bool {
  let renderer = try sharedRenderer()
  let view = NSView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
  let asked = programAsked ? 1 : 0
  let surface = try renderer.makeSurface(
    in: view,
    command: "/bin/sh -c \"printf '\\033[?1000h\\033[>\(asked)s%s-over-me\\n' drag; sleep 10\"")
  surface.setSize(width: 800, height: 600)
  var reports: [SearchReport] = []
  surface.onSearch = { reports.append($0) }

  // The row is found before it is dragged over, because the bytes reach the surface through its
  // pty some time after it exists, and they carry the request ahead of the text. A drag over an
  // empty screen selects nothing either way, which would pass the half of this that expects none.
  try await answered("the row to be drawn") {
    surface.search(nil)
    surface.search("drag-over-me")
    return await polled(within: .milliseconds(250)) { reports.contains(.total(1)) }
  }
  surface.search(nil)

  surface.mouseMoved(to: NSPoint(x: 4, y: 6), modifiers: .shift)
  surface.mouseButton(0, pressed: true, modifiers: .shift)
  surface.mouseMoved(to: NSPoint(x: 90, y: 6), modifiers: .shift)
  surface.mouseButton(0, pressed: false, modifiers: .shift)
  return surface.selectedText?.isEmpty == false
}

/// The one runtime this process has, made by whichever test asks first.
@MainActor private var shared: Renderer?

@MainActor
private func sharedRenderer() throws -> Renderer {
  if let shared { return shared }
  let renderer = try Renderer(configPath: NSTemporaryDirectory() + "muster-surface.conf")
  Renderer.current = renderer
  shared = renderer
  return renderer
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
