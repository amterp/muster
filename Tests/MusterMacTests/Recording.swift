import AppKit
import Testing

@testable import MusterMac

// The seam and the input events, recorded rather than crossed.
//
// One home for both, because three suites want them and three copies of a decoder is three
// places to fix when the seam changes shape. What a suite still owns is its own fixtures - the
// view it builds and the gesture it drives.

/// Answers every request with `ok` and keeps what it was asked.
final class RecordingDispatcher: Dispatcher, @unchecked Sendable {
  private(set) var requests: [Muster_Request] = []

  func dispatch(_ request: [UInt8]) -> [UInt8] {
    if let decoded = try? Muster_Request(serializedBytes: request) {
      requests.append(decoded)
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

/// Points the seam at a fresh recorder.
@MainActor
func recorder() -> RecordingDispatcher {
  let recorder = RecordingDispatcher()
  Core.dispatcher = recorder
  return recorder
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
