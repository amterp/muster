import Testing

@testable import MusterMac

@Suite("a view with a newer one behind it")
struct ViewSequenceTests {
  @Test("is not applied, and the newest one is")
  func onlyTheNewestViewIsApplied() {
    let views = ViewSequence()
    let first = views.next(for: "window-1")
    let second = views.next(for: "window-1")
    let third = views.next(for: "window-1")
    #expect(!views.isLatest(first, for: "window-1"))
    #expect(!views.isLatest(second, for: "window-1"))
    #expect(views.isLatest(third, for: "window-1"))

    let fourth = views.next(for: "window-1")
    #expect(
      !views.isLatest(third, for: "window-1"), "a view stays the newest once another has arrived")
    #expect(views.isLatest(fourth, for: "window-1"))
  }

  @Test("for one window does not stop another window's view being applied")
  func eachWindowHasItsOwnNewest() {
    let views = ViewSequence()
    let ours = views.next(for: "window-1")
    let theirs = views.next(for: "window-2")
    #expect(views.isLatest(ours, for: "window-1"), "a view for another window replaced this one")
    #expect(views.isLatest(theirs, for: "window-2"))
  }
}
