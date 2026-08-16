import AppKit
import Testing

@testable import MusterMac

// What the find bar asks of the core, and what it asks of the renderer.
//
// The division of labour is the thing worth protecting here. The core searches - it is the
// only party that can, since a pane's history is the daemon's and a surface is repainted from
// frames - and the renderer only marks what is on screen. So these assert on the requests that
// cross the seam and on what the surface was told to mark, and never on a count, which is the
// core's answer and is judged by its own corpus.

/// A dispatcher that answers a find the way the core does, and records what it was asked.
private final class FindingDispatcher: Dispatcher, @unchecked Sendable {
  private let lock = NSLock()
  private var recorded: [Muster_Request] = []

  /// What every find and step will be answered with.
  private let answer: Muster_Findings

  init(total: UInt32 = 0, selected: UInt32 = 0, rows: UInt32 = 0, truncated: Bool = false) {
    var findings = Muster_Findings()
    findings.total = total
    findings.selected = selected
    findings.rowsSearched = rows
    findings.truncated = truncated
    answer = findings
  }

  var requests: [Muster_Request] { lock.withLock { recorded } }

  func dispatch(_ request: [UInt8]) -> [UInt8] {
    guard let decoded = try? Muster_Request(serializedBytes: request) else { return [] }
    lock.withLock { recorded.append(decoded) }
    var response = Muster_Response()
    switch decoded.payload {
    case .find, .findStep: response.findings = answer
    default: response.ok = Muster_Ok()
    }
    return (try? response.serializedBytes()) ?? []
  }

  func needles() -> [String] {
    requests.compactMap { if case .find(let find) = $0.payload { find.needle } else { nil } }
  }

  func steps() -> [String] {
    requests.compactMap {
      if case .findStep(let step) = $0.payload { step.direction } else { nil }
    }
  }

  var endedFinds: Int {
    requests.filter { if case .endFind = $0.payload { true } else { false } }.count
  }
}

@MainActor
private func chrome(_ surface: RecordingSurface) -> PaneChrome {
  let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
  view.attach(surface, typeable: true)
  return PaneChrome(frame: NSRect(x: 0, y: 0, width: 400, height: 300), surface: view)
}

@Suite("find")
struct FindTests {
  @MainActor
  @Test("a step names its direction, and does nothing with no bar up")
  func aStepNamesItsDirection() {
    // Two actions rather than keys the bar swallows, so they work with the keyboard back in
    // the pane. Which means they can be pressed with no bar at all, and that has to cost
    // nothing rather than ask the core to walk a search it is not holding.
    let core = FindingDispatcher(total: 3, selected: 2)
    Core.dispatcher = core
    let bar = FindBar(dispatcher: core)

    bar.step(forward: true)
    #expect(core.steps().isEmpty, "a step with no bar up reached the core")

    bar.show(over: chrome(RecordingSurface()))
    bar.step(forward: true)
    bar.step(forward: false)

    #expect(core.steps() == ["next", "previous"])
  }

  @MainActor
  @Test("closing tells the core to forget, and takes the marks off the pane")
  func closingForgets() {
    // Both halves matter and they fail differently. A core still holding a search answers a
    // later step about a pane nobody is looking at; a pane still marked is a terminal with
    // yellow text in it and nothing on screen explaining why.
    let core = FindingDispatcher()
    Core.dispatcher = core
    let surface = RecordingSurface()
    let bar = FindBar(dispatcher: core)
    bar.show(over: chrome(surface))

    bar.close()

    #expect(core.endedFinds == 1)
    #expect(surface.highlighted.last == .some(nil), "the pane kept its marks after closing")
    #expect(!bar.isShown)
  }

  @MainActor
  @Test("following the keyboard to another pane unmarks the one it left")
  func movingUnmarksTheOldPane() {
    // The find bar follows the keyboard, because a find is about a pane. Without this the
    // pane left behind stays marked, so two panes look searched and only one is counted.
    let core = FindingDispatcher()
    Core.dispatcher = core
    let first = RecordingSurface()
    let second = RecordingSurface()
    let bar = FindBar(dispatcher: core)

    bar.show(over: chrome(first))
    bar.show(over: chrome(second))

    #expect(first.highlighted.last == .some(nil), "the pane the bar left kept its marks")
    #expect(bar.isShown)
  }

  @MainActor
  @Test("a needle reaches the core, and only the last one typed does")
  func typingCoalescesBehindTheRoundTrip() async {
    // The reason a needle leaves through a sender at all. A find is a round trip - the core
    // reads the pane's history back before it can match anything - and it happens once per
    // keystroke, which over an ssh-forwarded socket is tens of milliseconds a character.
    let core = FindingDispatcher(total: 2, selected: 1)
    let sender = FindSender(dispatcher: core)

    sender.send(needle: "e")
    sender.send(needle: "er")
    sender.send(needle: "err")

    await until("the needle to reach the core") { !core.needles().isEmpty }
    await until("the last needle typed to reach the core") { core.needles().last == "err" }
    #expect(core.needles().count < 3, "every keystroke went out rather than the latest")
  }

  @MainActor
  @Test("a surface with nothing rendering it is asked for nothing")
  func aDetachedSurfaceIsNotAsked() {
    // A pane whose bridge has not started has nothing on screen to mark. Answering with a
    // refusal there would report a renderer problem for a pane with no renderer yet, which
    // is the ordinary state at launch.
    let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))

    #expect(view.highlight("error").isEmpty)
  }

  @MainActor
  @Test("what the renderer would not do is carried back, not swallowed")
  func aRefusedMarkIsReported() {
    // The failure this can have: the action is a string libghostty parses, so a version that
    // renamed it is a highlight that quietly stops appearing while every count stays right.
    let surface = RecordingSurface()
    surface.refuses = ["search:error"]
    let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
    view.attach(surface, typeable: true)

    #expect(view.highlight("error") == ["search:error"])
    #expect(surface.highlighted == ["error"])
  }
}
