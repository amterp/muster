import Testing

@testable import MusterMac

@Suite("a view with a newer one behind it")
struct ViewSequenceTests {
  @Test("is not applied, and the newest one is")
  func onlyTheNewestViewIsApplied() {
    let views = ViewSequence()
    let first = views.next()
    let second = views.next()
    let third = views.next()
    #expect(!views.isLatest(first))
    #expect(!views.isLatest(second))
    #expect(views.isLatest(third))

    let fourth = views.next()
    #expect(!views.isLatest(third), "a view stays the newest once another has arrived")
    #expect(views.isLatest(fourth))
  }
}
