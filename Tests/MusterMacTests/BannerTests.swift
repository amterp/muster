import AppKit
import Testing

@testable import MusterMac

// What the shell does with the core's banners: what it keeps of a reply being written, what it
// tells the core, and how it words a time. What stands and what a reply came to are the core's
// and are tested there (corpus/conformance/banners.json); these are the wiring.

@MainActor
func recordedBoard() -> (BannerBoard, BannerRecorder) {
  let board = BannerBoard()
  let recorder = BannerRecorder()
  board.core = recorder.requests
  board.postedLinger = .milliseconds(10)
  return (board, recorder)
}

@MainActor
final class BannerRecorder {
  var sent: [String] = []

  var requests: BannerRequests {
    BannerRequests(
      reply: { [self] in sent.append("reply \($0)/\($1)#\($2): \($3)") },
      dismiss: { [self] in sent.append("dismiss \($0)/\($1)#\($2)") },
      hold: { [self] in sent.append("hold \($0)/\($1)#\($2) \($3)") },
      pick: { [self] in sent.append("pick \($0)/\($1)#\($2) \($3.map(String.init) ?? "none")") },
      open: { [self] in sent.append("open \($0)/\($1)") })
  }
}

func aBanner(
  _ group: String, seq: UInt64, author: String = "otter", sent: Muster_PostOutcome? = nil,
  answers: [String] = [], picked: UInt32? = nil
) -> Muster_Banner {
  var banner = Muster_Banner()
  banner.daemonID = "local"
  banner.group = group
  banner.seq = seq
  banner.author = author
  banner.replyable = true
  banner.answers = answers
  if let picked { banner.picked = picked }
  if let sent { banner.sent = sent }
  return banner
}

private func outcome(_ state: Muster_PostOutcome.State, said: String = "", body: String = "")
  -> Muster_PostOutcome
{
  var outcome = Muster_PostOutcome()
  outcome.state = state
  outcome.said = said
  outcome.heard = true
  outcome.body = body
  return outcome
}

func bannerStack(_ banners: Muster_Banner...) -> Muster_BannersChanged {
  var changed = Muster_BannersChanged()
  changed.shown = banners
  return changed
}

@Suite("the banners for messages")
struct BannerTests {
  @MainActor
  @Test("the core hears that a reply is being written only when the field fills or empties")
  func holdingFollowsTheFieldFillingAndEmptying() {
    let (board, recorder) = recordedBoard()
    let shown = aBanner("g", seq: 4)
    board.apply(bannerStack(shown))

    board.setReply("y", for: shown)
    board.setReply("ye", for: shown)
    board.setReply("yes", for: shown)
    board.setReply("", for: shown)

    #expect(recorder.sent == ["hold local/g#4 true", "hold local/g#4 false"])
  }

  @MainActor
  @Test("a reply goes to the banner it was written in, and an empty one is not sent")
  func sendingNamesTheBannersMessage() {
    let (board, recorder) = recordedBoard()
    let shown = aBanner("g", seq: 4)
    board.apply(bannerStack(shown))

    board.send(shown)
    board.setReply("yes", for: shown)
    board.send(shown)

    #expect(recorder.sent == ["hold local/g#4 true", "reply local/g#4: yes"])
  }

  @MainActor
  @Test("a refused reply keeps its words, and a posted one clears them")
  func theWordsStayUntilTheReplyIsPosted() {
    let (board, _) = recordedBoard()
    board.apply(bannerStack(aBanner("g", seq: 4)))
    board.apply(
      bannerStack(aBanner("g", seq: 4, sent: outcome(.refused, said: "a left g", body: "yes"))))
    #expect(
      board.reply(for: aBanner("g", seq: 4)) == "yes", "the core kept the words, so does the field")

    board.apply(bannerStack(aBanner("g", seq: 4, sent: outcome(.posted, said: "woke a"))))
    #expect(board.reply(for: aBanner("g", seq: 4)) == "")
  }

  @MainActor
  @Test("a reply that woke somebody says so for a moment and the banner comes down")
  func aHeardReplyLingersThenDismisses() async {
    let (board, recorder) = recordedBoard()
    board.apply(bannerStack(aBanner("g", seq: 4, sent: outcome(.posted, said: "woke a"))))

    for _ in 0..<100 where recorder.sent.isEmpty {
      try? await Task.sleep(for: .milliseconds(10))
    }

    #expect(recorder.sent == ["dismiss local/g#4"])
  }

  @MainActor
  @Test("picking an answer asks the core, and picking the picked one takes it back")
  func pickingAnAnswerTogglesItThroughTheCore() {
    let (board, recorder) = recordedBoard()
    let question = aBanner("g", seq: 4, answers: ["Run it now", "Skip it"])
    board.apply(bannerStack(question))

    board.pick(1, in: question)
    let picked = aBanner("g", seq: 4, answers: ["Run it now", "Skip it"], picked: 1)
    board.apply(bannerStack(picked))
    board.pick(1, in: picked)
    board.pick(0, in: picked)
    board.unpick(picked)

    #expect(
      recorder.sent == [
        "pick local/g#4 1", "pick local/g#4 none", "pick local/g#4 0", "pick local/g#4 none",
      ])
  }

  @MainActor
  @Test("an answer picked with nothing written is a whole reply")
  func aPickedAnswerSendsWithNothingWritten() {
    let (board, recorder) = recordedBoard()
    let unpicked = aBanner("g", seq: 4, answers: ["Run it now", "Skip it"])
    board.apply(bannerStack(unpicked))
    board.send(unpicked)
    #expect(recorder.sent.isEmpty, "nothing written and nothing picked is no reply")

    let picked = aBanner("g", seq: 4, answers: ["Run it now", "Skip it"], picked: 0)
    board.apply(bannerStack(picked))
    board.send(picked)

    #expect(recorder.sent == ["reply local/g#4: "])
  }

  @MainActor
  @Test("what was written for a banner that came down is forgotten")
  func aGoneBannersReplyGoes() {
    let (board, _) = recordedBoard()
    let shown = aBanner("g", seq: 4)
    board.apply(bannerStack(shown))
    board.setReply("half a thought", for: shown)
    board.apply(bannerStack(shown, aBanner("h", seq: 1)))
    #expect(board.reply(for: shown) == "half a thought", "another banner arriving keeps it")

    board.apply(bannerStack())
    #expect(board.replies.isEmpty)
  }

  @MainActor
  @Test("the chord that sends is the core's binding, modifiers and all")
  func theSendChordIsTheCores() {
    let (board, _) = recordedBoard()
    #expect(!board.sends(key: "\r", modifiers: .command), "nothing sends before the core says")

    board.take(bindings: [
      Core.Binding(action: "zoom", key: "Enter", modifiers: ["shift", "super"]),
      Core.Binding(action: "send_message", key: "Enter", modifiers: ["super"], inTextField: true),
    ])

    #expect(board.sends(key: "\r", modifiers: .command))
    #expect(!board.sends(key: "\r", modifiers: []), "Return alone writes a new line")
    #expect(!board.sends(key: "\r", modifiers: [.command, .shift]))
    #expect(board.sends(key: "\r", modifiers: [.command, .capsLock]), "caps lock is no chord")
  }

  @MainActor
  @Test("Send Message is available only while a banner's reply field has the keyboard")
  func sendMessageNeedsAReplyField() {
    let send = #selector(MusterWindow.sendMessage(_:))
    #expect(!MusterWindow.isAvailable(send))
    #expect(MusterWindow.isAvailable(send, replying: true))
  }

  @MainActor
  @Test("Send Message gets no key equivalent and is still listed with its chord")
  func sendMessageIsListedButNotAMenuChord() {
    let bindings = [
      Core.Binding(action: "send_message", key: "Enter", modifiers: ["super"], inTextField: true)
    ]
    let item = AppMenu.paneItems(bindings).first

    #expect(item?.title == "Send Message")
    #expect(item?.key == "")
    let rows = Shortcuts.sections(bindings).flatMap(\.rows)
    #expect(rows.contains { $0.title == "Send Message" && $0.chord == "⌘↩" })
  }

  @Test("a time today is the time of day, and another day's carries its date")
  func timesAreTwentyFourHourWithSeconds() {
    let zone = TimeZone(secondsFromGMT: 2 * 3600)!
    let at: UInt64 = 1_792_852_083_123  // 2026-10-24 16:28:03.123 at UTC+2
    let sameDay = Date(timeIntervalSince1970: 1_792_860_000)
    let nextDay = Date(timeIntervalSince1970: 1_792_960_000)

    #expect(BannerTime.shown(atMs: at, now: sameDay, zone: zone) == "16:28:03")
    #expect(BannerTime.shown(atMs: at, now: nextDay, zone: zone) == "2026-10-24 16:28:03")
    #expect(BannerTime.exact(atMs: at, zone: zone) == "2026-10-24 16:28:03.123 +02:00")
  }

  @Test("a message's runs keep their styles and links")
  func runsBecomeStyledText() {
    var strong = Muster_MarkdownRun()
    strong.text = "now"
    strong.strong = true
    var link = Muster_MarkdownRun()
    link.text = "the MIP"
    link.link = "https://example.com/mip"
    var plain = Muster_MarkdownRun()
    plain.text = "read "

    let text = BannerMarkdown.attributed([plain, link, strong])

    #expect(String(text.characters) == "read the MIPnow")
    let runs = text.runs.map {
      (String(text[$0.range].characters), $0.link, $0.inlinePresentationIntent)
    }
    #expect(runs.count == 3)
    #expect(runs[1].1 == URL(string: "https://example.com/mip"))
    #expect(runs[2].2 == .stronglyEmphasized)
  }
}

@Suite("opening a group", .ownsTheSeam)
@MainActor
struct OpeningAGroupTests {
  /// The banner's Open, its folded count, the sidebar's row and the operating system's
  /// notification all call `Core.openConversation`, so this is what each of them asks for.
  @Test("opening a group asks the core to show its conversation")
  func openingShowsTheConversation() {
    let recorder = recorder()

    Core.openConversation(daemonID: "devenv", group: "review")

    let shown = recorder.requests.compactMap {
      if case .showConversation(let show) = $0.payload { show } else { nil }
    }
    #expect(shown.map { "\($0.daemonID)/\($0.group)" } == ["devenv/review"])
  }
}
