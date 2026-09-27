import Testing

@testable import MusterMac

// A held paste is one a daemon would not send without a yes, because each line of it would run.
// What has to hold: a yes sends it once, confirmed and into the pane that held it; a no sends
// nothing; and two held at once are each asked about rather than one dropping the other.

/// Stands in for the sheet: keeps each question and its answer, for the test to give.
@MainActor
private final class Asked {
  var held: [HeldPaste] = []
  var answers: [(Bool) -> Void] = []

  lazy var pastes = HeldPastes { [unowned self] held, answer in
    self.held.append(held)
    self.answers.append(answer)
  }
}

private func pastes(_ recorder: RecordingDispatcher) -> [Muster_Paste] {
  recorder.requests.compactMap { if case .paste(let paste) = $0.payload { paste } else { nil } }
}

@Suite("held pastes", .ownsTheSeam)
@MainActor
struct HeldPastesTests {
  private let pane = PaneKey(daemon: "devenv", pane: "p1w3r07bsd")

  @Test("a yes sends the paste once, confirmed, into the pane that held it")
  func confirmingSendsIt() {
    let recorder = recorder()
    let asked = Asked()

    asked.pastes.hold(HeldPaste(pane: pane, text: "make\nmake install\n"))
    asked.answers[0](true)

    let sent = pastes(recorder)
    #expect(sent.count == 1)
    #expect(sent.first?.text == "make\nmake install\n")
    #expect(sent.first?.confirmed == true)
    // The keyboard may have moved while somebody read the question, so the pane is named.
    #expect(sent.first?.daemonID == "devenv")
    #expect(sent.first?.paneID == "p1w3r07bsd")
  }

  @Test("a no sends nothing")
  func cancellingSendsNothing() {
    let recorder = recorder()
    let asked = Asked()

    asked.pastes.hold(HeldPaste(pane: pane, text: "rm -rf build\n"))
    asked.answers[0](false)

    #expect(pastes(recorder).isEmpty)
  }

  @Test("a second held paste waits for the first answer, and is asked about after it")
  func heldPastesQueue() {
    let recorder = recorder()
    let asked = Asked()

    asked.pastes.hold(HeldPaste(pane: pane, text: "first\n"))
    asked.pastes.hold(HeldPaste(pane: pane, text: "second\n"))
    #expect(asked.held.map(\.text) == ["first\n"], "two questions were up at once")

    asked.answers[0](false)
    #expect(asked.held.map(\.text) == ["first\n", "second\n"])

    asked.answers[1](true)
    #expect(pastes(recorder).map(\.text) == ["second\n"])
  }
}
