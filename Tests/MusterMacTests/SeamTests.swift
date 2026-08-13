import Testing

@testable import MusterMac

// The link, and the round trip across it.
//
// Small on purpose, and load-bearing out of proportion to its size: every other test in
// this suite runs against a fake, so this is the only one that proves a Swift shell can
// reach a Rust core at all. If the two libghostty builds ever stop coexisting - the
// surface xcframework statically, libghostty-vt behind this dylib - the failure lands
// here rather than in a window that will not open.

@Test func theCoreAnswersARequest() {
  var request = Muster_Request()
  request.startup = Muster_Startup()

  let response = CoreDispatcher().dispatch(try! request.serializedBytes())

  #expect(try! Muster_Response(serializedBytes: response).payload == .ok(Muster_Ok()))
}

@Test func aRefusalCrossesBackIntact() {
  // The failure direction is the half that goes untested by accident, and the half that
  // matters at 2am: a core that refuses has to be distinguishable from one that is gone.
  var startup = Muster_Startup()
  startup.logPath = "/nonexistent-directory-for-a-test/muster.jsonl"
  var request = Muster_Request()
  request.startup = startup

  let response = try! Muster_Response(
    serializedBytes: CoreDispatcher().dispatch(try! request.serializedBytes()))

  guard case .failure(let failure) = response.payload else {
    Issue.record("expected a refusal, got \(response.payload as Any)")
    return
  }
  #expect(failure.reason.contains("/nonexistent-directory-for-a-test/muster.jsonl"))
}

@Test func anEmptyRequestIsAnsweredRatherThanCrashing() {
  // A null pointer and a zero length reach the core as the same thing, and the boundary
  // has to survive both. This is the shape of every "it worked until the first edge case"
  // FFI bug.
  #expect(!CoreDispatcher().dispatch([]).isEmpty)
}
