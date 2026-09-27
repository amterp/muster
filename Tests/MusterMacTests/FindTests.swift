import AppKit
import MusterRenderer
import Testing

@testable import MusterMac

// What the find bar asks of the pane's surface, and what it draws from the answers.
//
// The surface searches - it holds the pane's history - so these assert on what it was asked to
// search for and on what the bar drew from what it reported, and never on a count of real
// matches, which is libghostty's to get right.

@MainActor
private func chrome(_ surface: RecordingSurface) -> PaneChrome {
  let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
  view.attach(surface, typeable: true)
  let chrome = PaneChrome(frame: NSRect(x: 0, y: 0, width: 400, height: 300), surface: view)
  chrome.attach(paneID: "p1w3r07bsd")
  return chrome
}

@Suite("find")
struct FindTests {
  @MainActor
  @Test("a needle of three characters or more is searched for at once")
  func aLongNeedleGoesStraightOut() {
    let surface = RecordingSurface()
    let bar = FindBar()
    let pane = chrome(surface)
    bar.show(over: pane)

    bar.typed("err")

    #expect(surface.searches == ["err"])
  }

  @MainActor
  @Test("a short needle waits, and only the last one typed is searched for")
  func shortNeedlesAreDebounced() async {
    // One or two characters match nearly everything in a pane's history, and a longer needle
    // is typed through them. Searching each would cost two searches of the whole history for
    // every needle anybody types.
    let surface = RecordingSurface()
    let bar = FindBar()
    let pane = chrome(surface)
    bar.show(over: pane)

    bar.typed("e")
    bar.typed("er")
    #expect(surface.searches.isEmpty, "a short needle went out without waiting")

    await until("the short needle to be searched for") { !surface.searches.isEmpty }
    #expect(surface.searches == ["er"])
    // The bar holds its pane weakly, and a pane gone by the time the wait ends is searched for
    // nothing.
    withExtendedLifetime(pane) {}
  }

  @MainActor
  @Test("typing past a short needle cancels it")
  func aLongerNeedleCancelsTheWait() async throws {
    // Otherwise the short needle lands after the long one and replaces it, and the bar counts
    // matches of "e" under a field reading "err".
    let surface = RecordingSurface()
    let bar = FindBar()
    let pane = chrome(surface)
    bar.show(over: pane)

    bar.typed("e")
    bar.typed("err")
    try await Task.sleep(for: FindBar.debounce * 2)

    #expect(surface.searches == ["err"])
  }

  @MainActor
  @Test("emptying the field stops the search at once")
  func anEmptyNeedleIsNotDebounced() {
    // An empty needle matches nothing and costs nothing, and the marks of the last one should
    // leave with the text.
    let surface = RecordingSurface()
    let bar = FindBar()
    let pane = chrome(surface)
    bar.show(over: pane)

    bar.typed("")

    #expect(surface.searches == [""])
  }

  @MainActor
  @Test("the counter draws what the surface reported")
  func theCounterFollowsTheReports() {
    let surface = RecordingSurface()
    let bar = FindBar()
    let pane = chrome(surface)
    bar.show(over: pane)
    bar.typed("error")
    #expect(bar.state.counter == "", "a count was drawn before the surface reported one")

    surface.onSearch?(.total(47))
    #expect(bar.state.counter == "-/47")

    surface.onSearch?(.selected(2))
    #expect(bar.state.counter == "3/47")

    // What libghostty sends while it does not know. The place is still worth showing.
    surface.onSearch?(.total(nil))
    #expect(bar.state.counter == "3/?")
  }

  @MainActor
  @Test("a new needle forgets the old one's counts")
  func aNewNeedleClearsTheCounter() {
    // Until the renderer answers, the counts on screen are for the needle before, drawn under
    // the one now in the field.
    let surface = RecordingSurface()
    let bar = FindBar()
    let pane = chrome(surface)
    bar.show(over: pane)
    bar.typed("error")
    surface.onSearch?(.total(47))
    surface.onSearch?(.selected(2))

    bar.typed("errors")

    #expect(bar.state.counter == "")
  }

  @MainActor
  @Test("a step names its direction, and does nothing with no bar up")
  func aStepNamesItsDirection() {
    // Two actions rather than keys the bar swallows, so they work with the keyboard back in
    // the pane. Which means they can be pressed with no bar at all, and that has to cost
    // nothing.
    let surface = RecordingSurface()
    let bar = FindBar()

    bar.step(forward: true)
    #expect(surface.navigations.isEmpty)

    let pane = chrome(surface)
    bar.show(over: pane)
    bar.step(forward: true)
    bar.step(forward: false)

    #expect(surface.navigations == [true, false])
  }

  @MainActor
  @Test("closing ends the search, and stops listening to the pane")
  func closingEndsTheSearch() {
    // A pane still searched is a terminal with marked text in it and nothing on screen
    // explaining why.
    let surface = RecordingSurface()
    let bar = FindBar()
    let pane = chrome(surface)
    bar.show(over: pane)
    bar.typed("error")
    surface.onSearch?(.total(3))

    bar.close()

    #expect(surface.searches.last == .some(nil), "the pane's search outlived the bar")
    #expect(bar.state.counter == "")
    #expect(!bar.isShown)
    surface.onSearch?(.total(9))
    #expect(bar.state.counter == "", "the closed bar still takes the pane's counts")
  }

  @MainActor
  @Test("closing cancels a short needle still waiting")
  func closingCancelsTheWait() async throws {
    let surface = RecordingSurface()
    let bar = FindBar()
    let pane = chrome(surface)
    bar.show(over: pane)
    bar.typed("e")

    bar.close()
    try await Task.sleep(for: FindBar.debounce * 2)

    #expect(surface.searches == [nil])
  }

  @MainActor
  @Test("following the keyboard to another pane searches it, and ends the one it left")
  func movingSearchesTheNewPane() {
    // The find bar follows the keyboard, because a find is about a pane. The pane left behind
    // would otherwise stay marked, so two panes look searched and only one is counted.
    let first = RecordingSurface()
    let second = RecordingSurface()
    let bar = FindBar()
    let firstPane = chrome(first)
    bar.show(over: firstPane)
    // The field's binding sets this before it reports the keystroke, and there is no field here.
    bar.state.needle = "error"
    bar.typed("error")

    let secondPane = chrome(second)
    bar.show(over: secondPane)

    #expect(first.searches.last == .some(nil), "the pane the bar left is still searched")
    #expect(second.searches == ["error"])
    #expect(bar.isShown)
  }

  @MainActor
  @Test("a needle the renderer would not search for is reported, not swallowed")
  func aRefusedNeedleIsReported() {
    // The failure this can have: the action is a string libghostty parses, so a version that
    // renamed it is a find that quietly counts nothing.
    let surface = RecordingSurface()
    surface.refuses = ["search:error"]
    let bar = FindBar()
    var reported: [[String]] = []
    bar.onRefused = { reported.append($0) }
    let pane = chrome(surface)
    bar.show(over: pane)

    bar.typed("error")

    #expect(reported == [["search:error"]])
  }

  @MainActor
  @Test("a surface with nothing rendering it is asked for nothing")
  func aDetachedSurfaceIsNotAsked() {
    // A pane whose bridge has not started has nothing to search. Answering with a refusal
    // there would report a renderer problem for a pane with no renderer yet, which is the
    // ordinary state at launch.
    let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))

    #expect(view.search("error").isEmpty)
  }

  @MainActor
  @Test("a report reaches a bar that opened before the pane's surface did")
  func reportsSurviveALateSurface() {
    // The bar listens to the view, and the view is handed its surface when the bridge starts,
    // which can be after somebody pressed the chord.
    let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
    let chrome = PaneChrome(frame: NSRect(x: 0, y: 0, width: 400, height: 300), surface: view)
    chrome.attach(paneID: "p1w3r07bsd")
    let bar = FindBar()
    bar.show(over: chrome)
    let surface = RecordingSurface()

    view.attach(surface, typeable: true)
    surface.onSearch?(.total(5))

    #expect(bar.state.counter == "-/5")
  }
}
