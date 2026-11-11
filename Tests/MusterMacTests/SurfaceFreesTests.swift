import Foundation
import Testing

@testable import MusterRenderer

/// A renderer going away while a surface it freed is still finishing hands its app to
/// `SurfaceFrees` rather than waiting for the free: the free finishes in a main-queue job, so a
/// deinit reached from another main-queue job that waited for it would wait out any bound it had
/// and leak the app.
@MainActor
@Suite("freeing the app behind surfaces still being freed")
struct SurfaceFreesTests {
  final class Calls {
    var ticks = 0
    var releases = 0
  }

  @Test("an app with no free in flight is released at once")
  func releasedAtOnce() {
    let frees = SurfaceFrees()
    let calls = Calls()
    frees.release(ticking: { calls.ticks += 1 }, then: { calls.releases += 1 })
    #expect(calls.releases == 1)
    #expect(calls.ticks == 0)
  }

  @Test("a teardown started inside a main-queue job is released by a free that ends after it")
  func releasedAfterTheLastFree() async throws {
    let frees = SurfaceFrees()
    let calls = Calls()
    frees.began()

    await withCheckedContinuation { (started: CheckedContinuation<Void, Never>) in
      DispatchQueue.main.async {
        MainActor.assumeIsolated {
          frees.release(ticking: { calls.ticks += 1 }, then: { calls.releases += 1 })
          // Still inside the job: nothing has been released, and nothing waited.
          #expect(calls.releases == 0)
        }
        started.resume()
      }
    }

    try await until("the handed-over app to be ticked") { calls.ticks > 0 }
    #expect(calls.releases == 0)

    await withCheckedContinuation { (ended: CheckedContinuation<Void, Never>) in
      DispatchQueue.main.async {
        MainActor.assumeIsolated { frees.ended(onMainThread: Thread.isMainThread) }
        ended.resume()
      }
    }
    #expect(calls.releases == 1)
    #expect(frees.endedOnMainThread)

    let ticksAtRelease = calls.ticks
    try await Task.sleep(for: .milliseconds(50))
    #expect(calls.ticks == ticksAtRelease, "the app was still ticked after it was released")
    #expect(calls.releases == 1)
  }

  @Test("a free that ended off the main thread is remembered")
  func offMainThreadIsRemembered() {
    let frees = SurfaceFrees()
    frees.began()
    frees.ended(onMainThread: false)
    #expect(!frees.endedOnMainThread)
  }

  private func until(_ what: String, _ condition: @MainActor () -> Bool) async throws {
    let deadline = ContinuousClock.now + .seconds(5)
    while !condition() {
      guard ContinuousClock.now < deadline else {
        Issue.record("timed out waiting for \(what)")
        return
      }
      try await Task.sleep(for: .milliseconds(5))
    }
  }
}
