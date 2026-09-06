import AppKit
import MusterRenderer
import Testing

@testable import MusterMac

// The seam and the input events, recorded rather than crossed.
//
// One home for both, because three suites want them and three copies of a decoder is three
// places to fix when the seam changes shape. What a suite still owns is its own fixtures - the
// view it builds and the gesture it drives.

/// Records what a view asked of the thing rendering it, and answers with a fixed selection.
///
/// Here rather than in one suite because a real `Surface` wants a GPU, a window and a
/// libghostty runtime, so every suite about what the shell decides needs one of these - and
/// two copies would drift the moment `PaneSurface` grows a method.
@MainActor
final class RecordingSurface: PaneSurface {
  var positions: [NSPoint] = []
  var buttons: [Bool] = []
  var selectedText: String?
  var onProcessExited: (@MainActor (Bool) -> Void)?
  /// Every offset asked for, in order, so a test can tell "sized once" from "sized twice".
  var fontSizeOffsets: [Int32] = []
  /// In backing pixels, as libghostty answers. Nil is a surface nothing has sized yet.
  var cellPixelSize: (width: UInt32, height: UInt32)?
  /// Every needle it was asked to mark, `nil` for a clear, so a test can tell one from none.
  var highlighted: [String?] = []
  /// What this surface will not do, for the tests about a renderer that refuses.
  var refuses: [String] = []
  /// Every selection it was asked to draw, `nil` for a clear, so a test can tell one from
  /// none. Points rather than cells, because that is what the seam carries.
  var selections: [SurfaceSelection?] = []

  init(selection: String? = nil) { selectedText = selection }

  func setSize(width: UInt32, height: UInt32) {}
  func setFocus(_ focused: Bool) {}
  func setFontSizeOffset(_ points: Int32) -> [String] {
    fontSizeOffsets.append(points)
    return []
  }
  func highlight(_ text: String?) -> [String] {
    highlighted.append(text)
    return refuses
  }
  func mouseMoved(to point: NSPoint, modifiers: NSEvent.ModifierFlags) { positions.append(point) }
  func leftMouse(pressed: Bool, modifiers: NSEvent.ModifierFlags) { buttons.append(pressed) }
  func select(_ selection: SurfaceSelection?) {
    selections.append(selection)
    selectedText = selection == nil ? nil : selectedText
  }
}

/// Answers every request with `ok` and keeps what it was asked.
///
/// Locked, because not every request arrives on the thread that asked for it: a divider
/// position leaves on a background queue, so a test reading this while one is in flight would
/// otherwise be racing an array it is appending to.
final class RecordingDispatcher: Dispatcher, @unchecked Sendable {
  private let lock = NSLock()
  private var recorded: [Muster_Request] = []

  var requests: [Muster_Request] {
    lock.withLock { recorded }
  }

  func dispatch(_ request: [UInt8]) -> [UInt8] {
    if let decoded = try? Muster_Request(serializedBytes: request) {
      lock.withLock { recorded.append(decoded) }
    }
    var response = Muster_Response()
    response.ok = Muster_Ok()
    return (try? response.serializedBytes()) ?? []
  }
}

extension RecordingDispatcher {
  /// What the gesture sent, of the kind the test is about.
  ///
  /// Filtered rather than taken whole, because a gesture is not the only thing that reaches the
  /// recorder between two marks. Reads answered once for the life of the process - the
  /// divider's colour is one, and it fires the first time anything lays a divider out - land in
  /// whichever test happens to get there first, which is a different one on every run. Counting
  /// requests made those tests fail one run in four for a reason that had nothing to do with
  /// what they assert.
  func sent(since mark: Int, of kind: (Muster_Request) -> Bool) -> [Muster_Request] {
    requests.dropFirst(mark).filter(kind)
  }
}

/// A window's worth of pane surfaces, parked nowhere in particular.
///
/// Here rather than in one suite because both the region suite and the pane-action suite build
/// a region, and a region cannot be built without one. `startPane` defaults to doing nothing,
/// which is what a suite about what a region decides wants: a real one needs a GPU, a
/// libghostty runtime and a subprocess.
@MainActor
func paneSurfaces(
  startPane: @escaping PaneSurfaces.StartPane = { _, _, _, _, _ in }
) -> PaneSurfaces {
  PaneSurfaces(parkedIn: NSView(frame: .zero), startPane: startPane)
}

/// Points the seam at a fresh recorder, for the length of this test.
@MainActor
func recorder() -> RecordingDispatcher {
  seam(RecordingDispatcher())
}

/// Waits for something the main thread will do on its own, or says what it was waiting for.
///
/// For the one path that does not answer inline: a divider position goes to a background queue
/// and comes back on the main actor, so a test that asserted straight after asking would be
/// racing the round trip it started.
@MainActor
func until(
  _ what: String, within seconds: Double = 5, _ ready: @MainActor () -> Bool
) async {
  let deadline = Date().addingTimeInterval(seconds)
  while Date() < deadline {
    if ready() { return }
    try? await Task.sleep(nanoseconds: 1_000_000)
  }
  Issue.record("timed out after \(seconds)s waiting for \(what)")
}

/// A wheel notch, at the position the caller says.
///
/// Wheel events have no public constructor, so this goes through CGEvent, which does. The
/// location matters wherever a test drives a view hierarchy rather than one view: AppKit
/// hit-tests `scrollWheel` to whatever is under it.
func wheel(deltaY: CGFloat, at location: NSPoint = .zero) -> NSEvent? {
  guard
    let event = CGEvent(
      scrollWheelEvent2Source: nil, units: .line, wheelCount: 1, wheel1: 0, wheel2: 0, wheel3: 0)
  else { return nil }
  event.setDoubleValueField(.scrollWheelEventPointDeltaAxis1, value: Double(deltaY))
  event.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1, value: Double(deltaY))
  event.location = CGPoint(x: location.x, y: location.y)
  return NSEvent(cgEvent: event)
}

/// One test at a time may point the seam somewhere.
///
/// `Core.dispatcher` is one mutable global for the process, so a test that swaps it is writing
/// where every other test reads. On the face of it these tests are all `@MainActor` and so never
/// truly concurrent - but swift-testing runs them as tasks, and a task that awaits gives the
/// actor up, so a test with a single `await` in it can have another test's recorder installed
/// underneath it and go on asserting against the wrong core. That is what made the find test in
/// kan a_2JrhrSBOx flaky, and its symptom is the expensive kind: a failure that reads as the
/// feature under test being broken (kan a_2LMRCjcSV).
///
/// A gate rather than a per-task value, because the value is not the only thing that has to be
/// exclusive: what a test drives reaches the global from background queues and from views that
/// took it as a default argument long before, and none of those inherit a task-local. What is
/// held is the seam itself, for the length of one test.
///
/// Not an actor, because releasing has to be callable from a `defer` - which cannot await - so
/// that a test which throws still hands the seam back.
///
/// The cost is nothing that was really being spent. These tests share one actor already, so
/// what serializing takes away is interleaving at suspension points rather than parallelism -
/// and interleaving is the bug rather than the throughput.
private final class SeamGate: @unchecked Sendable {
  static let shared = SeamGate()

  private let lock = NSLock()
  private var held = false
  private var waiting: [CheckedContinuation<Void, Never>] = []

  func acquire() async {
    await withCheckedContinuation { continuation in
      lock.lock()
      if held {
        waiting.append(continuation)
        lock.unlock()
      } else {
        held = true
        lock.unlock()
        continuation.resume()
      }
    }
  }

  /// Synchronous, so the trait below can release from a `defer` and a test that throws still
  /// hands the seam back.
  func release() {
    lock.lock()
    if waiting.isEmpty {
      held = false
      lock.unlock()
      return
    }
    let next = waiting.removeFirst()
    lock.unlock()
    next.resume()
  }
}

/// Whether this test holds the seam, which is what `seam(_:)` refuses without.
///
/// A task-local rather than a flag on the gate, so the answer is about *this* test rather than
/// about whether some test somewhere is inside a scope.
enum SeamScope {
  @TaskLocal static var held = false
}

/// Gives one test the seam to itself, and puts back whatever was there before.
///
/// Applied as `@Test(.ownsTheSeam)`, or once on a suite whose tests all point the seam
/// somewhere. Restoring matters as much as the exclusion: outside every scope the global is the
/// real core again, so a test that reaches the seam without meaning to talks to the core rather
/// than to some other test's recorder and its assertions about "what was sent" stay its own.
struct OwnsTheSeam: TestTrait, SuiteTrait, TestScoping {
  /// So one annotation on a suite covers the suites inside it.
  var isRecursive: Bool { true }

  /// Nothing is scoped around a suite as a whole - only around each test in it. A suite-level
  /// scope would hold the gate for the length of the suite and then be asked for again by every
  /// test inside it, which is one test waiting on itself.
  func scopeProvider(for test: Test, testCase: Test.Case?) -> Self? {
    testCase == nil ? nil : self
  }

  func provideScope(
    for test: Test, testCase: Test.Case?, performing function: @Sendable () async throws -> Void
  ) async throws {
    try await withTheSeam { try await function() }
  }
}

extension Trait where Self == OwnsTheSeam {
  static var ownsTheSeam: Self { OwnsTheSeam() }
}

/// Runs something with the seam to itself, and puts back what it found.
///
/// What the trait above is made of, called directly only by the tests about the mechanism.
///
/// Reentrant, because an annotation on a suite and on a test inside it is an ordinary thing to
/// write and one test waiting for itself is not an ordinary thing to debug. The inner run skips
/// the gate - the same task is already holding it - and still puts back what it found, so a
/// scope always ends where it started however many it is inside.
func withTheSeam<Answer>(_ body: @Sendable () async throws -> Answer) async rethrows -> Answer {
  let alreadyHeld = SeamScope.held
  if !alreadyHeld {
    await SeamGate.shared.acquire()
  }
  let found = Core.dispatcher
  defer {
    Core.dispatcher = found
    if !alreadyHeld {
      SeamGate.shared.release()
    }
  }
  return try await SeamScope.$held.withValue(true) { try await body() }
}

/// Points the seam at this core for the rest of the test.
///
/// The one door, so that a test which swaps the global without saying so is found by the suite
/// rather than by a flake three weeks later. Recorded as an issue rather than trapped: the test
/// that forgot is the one that should fail, and taking the process down with it would destroy
/// every other test's output as well.
@MainActor
@discardableResult
func seam<Sender: Dispatcher>(_ sender: Sender) -> Sender {
  if !SeamScope.held {
    Issue.record(
      """
      This test points `Core.dispatcher` somewhere without holding the seam, so another test \
      running beside it can replace what it installed - and what fails then is whichever test \
      loses the race, reading as a broken feature rather than as a broken suite (kan \
      a_2LMRCjcSV). Add `.ownsTheSeam` to this test, or to the suite it is in.
      """)
  }
  Core.dispatcher = sender
  return sender
}
