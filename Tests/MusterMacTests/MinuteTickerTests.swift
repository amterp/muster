import Foundation
import Testing

@testable import MusterMac

@Suite("minute ticker")
struct MinuteTickerTests {
  /// What a window would hold: something that says whether a time is shown, and counts redraws.
  @MainActor
  private final class Window {
    var showsTime = false
    var ticks = 0
    lazy var ticker = MinuteTicker(
      needed: { [unowned self] in showsTime }, tick: { [unowned self] in ticks += 1 })
  }

  @Test("the minute is counted only while something shows a time")
  @MainActor func runsOnlyWhenNeeded() {
    let window = Window()
    window.ticker.reconsider()
    #expect(!window.ticker.isTicking)

    window.showsTime = true
    window.ticker.reconsider()
    #expect(window.ticker.isTicking)

    window.showsTime = false
    window.ticker.reconsider()
    #expect(!window.ticker.isTicking)
  }

  @Test("a tick redraws, and ends the timer once nothing shows a time any more")
  @MainActor func aTickRedrawsAndReconsiders() {
    let window = Window()
    window.showsTime = true
    window.ticker.reconsider()

    window.ticker.fire()
    #expect(window.ticks == 1)
    #expect(window.ticker.isTicking)

    window.showsTime = false
    window.ticker.fire()
    #expect(window.ticks == 2)
    #expect(!window.ticker.isTicking, "a state that stopped asking left a timer running")
  }

  @Test("a closed window ends its timer")
  @MainActor func stopEndsIt() {
    let window = Window()
    window.showsTime = true
    window.ticker.reconsider()
    window.ticker.stop()
    #expect(!window.ticker.isTicking)
  }
}
