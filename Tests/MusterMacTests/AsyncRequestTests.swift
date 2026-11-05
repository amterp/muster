import Foundation
import Testing

@testable import MusterMac

@Suite("daemon waits leave the main thread", .ownsTheSeam)
@MainActor
struct AsyncRequestTests {
  @Test("a conversation clears queued state without a published outcome", arguments: Reply.allCases)
  func conversationCompletionReleasesQueuedState(reply: Reply) async {
    let dispatcher = ReplyDispatcher(reply)
    _ = seam(dispatcher)
    let box = MessageBox(requests: .core(window: { "origin" }))
    var changed = Muster_MessageBoxChanged()
    changed.daemonID = "local"
    changed.group = "g"
    changed.draft.text = "send these words"
    changed.draft.sendable = true
    box.apply(changed)
    box.send()
    #expect(box.pending)
    await until("the conversation request returning") { !box.pending }
    #expect(
      dispatcher.recorder.requests.filter {
        if case .postFromConversation = $0.payload { true } else { false }
      }.count == 1)
  }

  @Test("a banner clears queued state even without a published outcome", arguments: Reply.allCases)
  func bannerCompletionReleasesQueuedState(reply: Reply) async {
    let dispatcher = ReplyDispatcher(reply)
    _ = seam(dispatcher)
    let board = BannerBoard()
    let banner = aBanner("g", seq: 4, answers: ["A", "B"], picked: 0)
    board.apply(bannerStack(banner))
    board.send(banner)
    #expect(board.pending(banner))
    await until("the banner request returning") { !board.pending(banner) }
    #expect(
      dispatcher.recorder.requests.filter {
        if case .replyToBanner = $0.payload { true } else { false }
      }.count == 1)
  }

  @Test("conversation and banner posts cannot overtake an earlier Pause")
  func postsFollowPause() async {
    let dispatcher = HoldingDispatcher { if case .loadEarlier = $0.payload { true } else { false } }
    _ = seam(dispatcher)
    defer { dispatcher.release() }
    let requests = ConversationRequests.core(window: { "conversation-window" })
    requests.loadEarlier(ConversationGroup(daemon: "slow", group: "A"))
    await until("the held page request") { dispatcher.requests.count == 1 }
    let group = ConversationGroup(daemon: "fast", group: "B")
    requests.pause(group, true)
    var posted = false
    requests.post(group) {
      MainActor.preconditionIsolated()
      posted = true
    }
    Core.replyToBanner(daemonID: group.daemon, group: group.group, seq: 7, body: "reply")
    #expect(dispatcher.requests.count == 1)
    #expect(!posted)
    dispatcher.release()
    await until("the ordered posts to return") { dispatcher.finished.count == 4 && posted }
    let sent = dispatcher.requests
    guard sent.count == 4 else {
      Issue.record("expected page, pause, conversation post and banner reply")
      return
    }
    #expect(sent[1].pauseGroup.group == "B" && sent[1].pauseGroup.paused)
    #expect(sent[2].postFromConversation.group == "B")
    #expect(sent[2].window == "conversation-window")
    #expect(sent[3].replyToBanner.group == "B" && sent[3].replyToBanner.body == "reply")
  }

  @Test("conversation waits leave pane creation free", arguments: [false, true])
  func conversationWaits(pausing: Bool) async {
    let dispatcher = HoldingDispatcher { request in
      switch request.payload {
      case .pauseGroup: return pausing
      case .loadEarlier: return !pausing
      default: return false
      }
    }
    _ = seam(dispatcher)
    defer { dispatcher.release() }
    let requests = ConversationRequests.core(window: { "conversation-window" })
    let group = ConversationGroup(daemon: "local", group: "review@remote")
    if pausing { requests.pause(group, true) } else { requests.loadEarlier(group) }
    await until("the held conversation request") { !dispatcher.requests.isEmpty }
    let sent = dispatcher.requests.first
    #expect(sent?.window == "conversation-window")
    if pausing {
      #expect(sent?.pauseGroup.daemonID == group.daemon)
      #expect(sent?.pauseGroup.group == group.group)
      #expect(sent?.pauseGroup.paused == true)
    } else {
      #expect(sent?.loadEarlier.daemonID == group.daemon)
      #expect(sent?.loadEarlier.group == group.group)
    }
    var created = false
    Core.createTab { _ in created = true }
    await until("pane creation during a held conversation request") { created }
    #expect(dispatcher.finished.count == 1)
    #expect(
      dispatcher.finished.allSatisfy { if case .createTab = $0.payload { true } else { false } })
    dispatcher.release()
    await until("the conversation worker returning") { dispatcher.finished.count == 2 }
  }

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
      dispatcher.requests.contains {
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
    let tab = dispatcher.requests.first {
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
    #expect(
      !other.requests.contains {
        if case .splitPane = $0.payload { return true }
        return $0.logRecord.event == "core.refused"
      })
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
  private let holding: @Sendable (Muster_Request) -> Bool
  private let lock = NSLock()
  private var returned: [Muster_Request] = []

  init(
    holding: @escaping @Sendable (Muster_Request) -> Bool = {
      if case .deleteGroup = $0.payload { true } else { false }
    }
  ) {
    self.holding = holding
  }

  var requests: [Muster_Request] {
    recorder.requests.filter { if case .logRecord = $0.payload { false } else { true } }
  }

  var finished: [Muster_Request] {
    lock.withLock { returned }.filter { if case .logRecord = $0.payload { false } else { true } }
  }

  func dispatch(_ request: [UInt8]) -> [UInt8] {
    let answer = recorder.dispatch(request)
    guard let decoded = try? Muster_Request(serializedBytes: request) else { return answer }
    if holding(decoded) {
      guard !Thread.isMainThread else {
        Issue.record("a daemon wait reached the main thread")
        return answer
      }
      gate.wait()
    }
    lock.withLock { returned.append(decoded) }
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
