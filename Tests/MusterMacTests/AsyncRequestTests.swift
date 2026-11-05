import Foundation
import Testing

@testable import MusterMac

@Suite("daemon waits leave the main thread", .ownsTheSeam)
@MainActor
struct AsyncRequestTests {
  @Test("a held group deletion leaves the main actor and pane creation free")
  func aGroupWaitDoesNotHoldCreation() async {
    let dispatcher = HoldingDispatcher()
    _ = seam(dispatcher)
    defer { dispatcher.release() }
    var deleted: Bool?
    var created = false

    Core.deleteGroup(daemonID: "local", group: "review@devenv") {
      MainActor.preconditionIsolated()
      deleted = $0
    }
    await until("the held group deletion") {
      dispatcher.recorder.requests.contains {
        if case .deleteGroup = $0.payload { true } else { false }
      }
    }
    Core.createTab(daemonID: "devenv", paneID: "remote-pane") { why in
      MainActor.preconditionIsolated()
      #expect(why == nil)
      created = true
    }
    await until("a tab completion while the group answer is held") { created }
    #expect(deleted == nil)
    let tab = dispatcher.recorder.requests.first {
      if case .createTab = $0.payload { true } else { false }
    }
    #expect(tab?.createTab.daemonID == "devenv")
    #expect(tab?.createTab.paneID == "remote-pane")

    dispatcher.release()
    await until("the group completion after its answer is released") { deleted != nil }
    #expect(deleted == true)
  }

  @Test("requests and their refusal logs keep the originating window and dispatcher")
  func theDestinationIsCapturedBeforeDispatch() async {
    let original = RecordingDispatcher { request in
      guard case .splitPane = request.payload else { return nil }
      var refusal = Muster_Failure()
      refusal.reason = "the pane is gone"
      return .failure(refusal)
    }
    let other = RecordingDispatcher()
    _ = seam(original)
    let previousWindow = Core.windowInFront
    defer { Core.windowInFront = previousWindow }
    var done = false
    Core.speaking(for: "window-origin") {
      Core.split(side: "down", daemonID: "devenv", paneID: "remote-pane") { why in
        MainActor.preconditionIsolated()
        #expect(why == "the pane is gone")
        done = true
      }
    }
    Core.windowInFront = "window-other"
    Core.dispatcher = other
    await until("the original dispatcher's refusal") { done }

    let split = original.requests.first { if case .splitPane = $0.payload { true } else { false } }
    #expect(split?.window == "window-origin")
    #expect(split?.splitPane.daemonID == "devenv")
    #expect(split?.splitPane.paneID == "remote-pane")
    #expect(original.requests.contains { $0.logRecord.event == "core.refused" })
    #expect(other.requests.isEmpty)
  }

  @Test(
    "a refusal, an unanswered request and a missing reply remain distinct",
    arguments: Reply.allCases)
  func outcomesReturnWithoutRetry(reply: Reply) async {
    let dispatcher = ReplyDispatcher(reply)
    _ = seam(dispatcher)
    var results = 0
    var created: String?
    var left: Bool?
    Core.createTab(daemonID: "remote", paneID: "source") { why in
      MainActor.preconditionIsolated()
      created = why
      results += 1
    }
    Core.leaveGroup(daemonID: "local", group: "review@remote") { taken in
      MainActor.preconditionIsolated()
      left = taken
      results += 1
    }
    await until("both asynchronous outcomes") { results == 2 }
    #expect(created == (reply == .refused ? "refused by daemon" : nil))
    #expect(left == (reply != .refused && reply != .missing))
    #expect(
      dispatcher.recorder.requests.filter {
        if case .createTab = $0.payload { true } else { false }
      }.count == 1)
    #expect(
      dispatcher.recorder.requests.filter {
        if case .leaveGroup = $0.payload { true } else { false }
      }.count == 1)
  }

  enum Reply: CaseIterable, Sendable {
    case success, refused, unanswered, missing
  }
}

private final class HoldingDispatcher: Dispatcher, @unchecked Sendable {
  let recorder = RecordingDispatcher()
  private let gate = DispatchSemaphore(value: 0)

  func dispatch(_ request: [UInt8]) -> [UInt8] {
    let answer = recorder.dispatch(request)
    if let decoded = try? Muster_Request(serializedBytes: request),
      case .deleteGroup = decoded.payload
    {
      guard !Thread.isMainThread else {
        Issue.record("a daemon wait reached the main thread")
        return answer
      }
      gate.wait()
    }
    return answer
  }

  func release() { gate.signal() }
}

private final class ReplyDispatcher: Dispatcher, Sendable {
  let recorder: RecordingDispatcher
  private let reply: AsyncRequestTests.Reply

  init(_ reply: AsyncRequestTests.Reply) {
    self.reply = reply
    recorder = RecordingDispatcher { request in
      if case .logRecord = request.payload { return nil }
      switch reply {
      case .refused:
        var failure = Muster_Failure()
        failure.reason = "refused by daemon"
        return .failure(failure)
      case .unanswered:
        var unanswered = Muster_Unanswered()
        unanswered.reason = "the daemon took the request and did not answer"
        return .unanswered(unanswered)
      case .success, .missing: return nil
      }
    }
  }

  func dispatch(_ request: [UInt8]) -> [UInt8] {
    let response = recorder.dispatch(request)
    return reply == .missing ? [] : response
  }
}
