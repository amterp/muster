import Foundation
import Testing

@testable import MusterMac

// The suite's own arrangements, tested because the suite is what everything else is trusted
// through. `Core.dispatcher` is one mutable global for the process, and a test that swaps it is
// writing where every other test reads - which is fine while nothing suspends, and a race the
// moment anything does. It was found the hard way: a find test stayed flaky until its author
// stopped awaiting between setting the global and using it, and the failure read as the feature
// under test being broken rather than as the suite tripping over itself (kan a_2LMRCjcSV).
//
// So the seam is held rather than merely set, and these are the three things that has to be
// true: one holder at a time, whatever was there is put back, and a test that reaches for the
// seam without holding it is told rather than left to flake.

/// Counts how many closures were inside at once, from whatever thread each ran on.
private final class Overlaps: @unchecked Sendable {
  private let lock = NSLock()
  private var inside = 0
  private var most = 0

  func entered() {
    lock.withLock {
      inside += 1
      most = max(most, inside)
    }
  }

  func left() {
    lock.withLock { inside -= 1 }
  }

  var mostAtOnce: Int {
    lock.withLock { most }
  }
}

@Suite("one test at a time may point the seam somewhere")
struct SeamScopeTests {
  @Test("four scopes asking at once go through one at a time")
  func onlyOneHolderAtATime() async {
    // With a suspension inside each, which is the whole condition: these tests share one actor,
    // so what they were ever racing over was a closure that gave the actor up in the middle and
    // came back to a seam somebody else had moved.
    let overlaps = Overlaps()
    await withTaskGroup(of: Void.self) { group in
      for _ in 0..<4 {
        group.addTask {
          await withTheSeam {
            overlaps.entered()
            try? await Task.sleep(nanoseconds: 2_000_000)
            overlaps.left()
          }
        }
      }
    }

    #expect(
      overlaps.mostAtOnce == 1,
      """
      two scopes held the seam at once, so one test's recorder can be installed under another \
      and the assertions of whichever loses read as a broken feature
      """)
  }

  @MainActor
  @Test("a scope puts back what it found", .ownsTheSeam)
  func aScopePutsBackWhatItFound() async {
    // Restoring matters as much as excluding, and it is the half nothing else would notice: a
    // scope that left its recorder installed would have every later test's stray request land
    // in a recorder belonging to a test that had already finished.
    let outer = recorder()
    let inner = RecordingDispatcher()

    await withTheSeam { await MainActor.run { seam(inner) } }
    Core.send(text: "after the inner scope")

    #expect(inner.requests.isEmpty, "the inner scope left the seam pointed at its own recorder")
    #expect(outer.requests.count == 1, "the seam did not come back to the scope around it")
  }

  @MainActor
  @Test("pointing the seam somewhere without holding it is reported", .ownsTheSeam)
  func swappingTheSeamOutsideAScopeIsReported() {
    // Enforced rather than written down, because the cost of forgetting is a flake three weeks
    // later in a test that has nothing to do with the one that forgot. This test holds the seam
    // so nothing is actually raced; what it drops is the scope's own answer to "is one held".
    withKnownIssue("a test that points the seam somewhere without holding it should be told") {
      SeamScope.$held.withValue(false) {
        seam(RecordingDispatcher())
      }
    }
  }
}
