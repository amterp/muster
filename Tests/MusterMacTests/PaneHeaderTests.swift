import AppKit
import Testing

@testable import MusterMac

// What a pane's header says, when it is there, and what it costs to keep right. The header draws
// the row the agent list would draw for the same pane, so what is pinned here is the choice of
// what to show: the chord, the state as a shape, who it is, how long it has held, and what it
// gives up as it narrows.

private let epoch = Date(timeIntervalSince1970: 1_000_000)

/// A pane's row as its header is handed it: `state` is what the agent is doing, `since` how long
/// ago it began, in seconds.
@MainActor
func testHeaderRow(
  _ id: String, state: String = "working", since: Double? = nil, tabPress: Int = 0,
  press: Int = 0, callsign: String = "otter", givenName: String = "",
  directory: String = "/Users/me/src/muster", context: Float? = nil, subtitle: String = "",
  now: Date = epoch
) -> SidebarModel.Row {
  let pane = Roster.Pane(
    key: PaneKey(daemon: "local", pane: id), place: press, tabPress: tabPress, press: press,
    label: givenName.isEmpty ? callsign : givenName, subtitle: subtitle, givenName: givenName,
    onScreen: true, callsign: callsign,
    besideCallsign: givenName.isEmpty ? "muster · claude" : givenName, directory: directory)
  var agent = PaneAgent(state: state)
  agent.sinceMS = since.map { Int64((now.timeIntervalSince1970 - $0) * 1000) } ?? 0
  agent.contextUsed = context
  return SidebarModel.paneRow(
    pane, in: "w1:t1", agent: agent, now: now, presses: (tabPress, press), onScreen: true)
}

@MainActor
private func header(
  width: CGFloat = 480, row: SidebarModel.Row
) -> PaneChrome {
  let frame = NSRect(x: 0, y: 0, width: width, height: 300)
  let chrome = PaneChrome(frame: frame, surface: SurfaceView(frame: frame))
  chrome.attach(paneID: row.pane?.pane)
  chrome.key = row.pane
  chrome.apply(header: row)
  chrome.layoutSubtreeIfNeeded()
  chrome.header.layoutSubtreeIfNeeded()
  return chrome
}

@Suite("pane headers")
@MainActor
struct PaneHeaderTests {
  @Test("a blocked pane's header carries its chord, a shape, who it is and how long it has waited")
  func aBlockedHeader() throws {
    let row = testHeaderRow("w1:p1", state: "blocked", since: 300, tabPress: 2, press: 3)
    let header = header(row: row).header

    #expect(header.shownKeys == [2, 3])
    #expect(header.shownIcon == "blocked")
    #expect(header.shownStateWord == "Blocked 5m")
    #expect(header.shownName.contains("otter"))
    #expect(header.showsTint, "blocked is tinted, as its row is")
  }

  @Test("a calm pane's header has no word and no tint, only where it is")
  func aCalmHeader() {
    let header = header(row: testHeaderRow("w1:p1", state: "working", since: 300, press: 4)).header

    #expect(header.shownKeys == [4], "a window of one tab names a pane with one press")
    #expect(header.shownIcon == "working")
    #expect(header.shownStateWord == nil)
    #expect(!header.showsTint)
  }

  @Test("only blocked and stuck are tinted, and done says so in words without it")
  func theTintFollowsTheTier() {
    for (state, tinted) in [
      ("blocked", true), ("stuck", true), ("done", false), ("working", false), ("idle", false),
      ("waiting", false), ("unknown", false),
    ] {
      let header = header(row: testHeaderRow("w1:p1", state: state, since: 120)).header
      #expect(header.showsTint == tinted, "\(state)")
    }
    let done = header(row: testHeaderRow("w1:p1", state: "done", since: 120)).header
    #expect(done.shownStateWord == "Done 2m")
  }

  @Test("the tint is the palette's header tint for the state, so its text keeps its contrast")
  func theTintIsThePalettes() throws {
    let header = header(row: testHeaderRow("w1:p1", state: "stuck", since: 60)).header
    let palette = ChromePalette.current(for: header.effectiveAppearance).header
    let tint = try #require(header.tintColor)
    #expect(RGB(tint).hex == palette.tint["stuck"]?.hex)
  }

  @Test("the directory is on a named pane's header and not on an unnamed one's")
  func theDirectoryOnlyWhenNamed() {
    let named = header(
      row: testHeaderRow("w1:p1", givenName: "Qm storage", directory: "/src/quartermaster")
    ).header
    #expect(named.showsDirectory)

    // An unnamed pane's name is its directory and harness already.
    let unnamed = header(row: testHeaderRow("w1:p1", directory: "/src/quartermaster")).header
    #expect(!unnamed.showsDirectory)
  }

  @Test("a header gives up its directory and then its context bar as it narrows")
  func narrowing() {
    let named = testHeaderRow("w1:p1", givenName: "Qm storage", context: 40)
    let wide = PaneHeaderModel.parts(width: 400, for: named)
    #expect(wide == PaneHeaderModel.Parts(directory: true, context: true))
    let medium = PaneHeaderModel.parts(width: 300, for: named)
    #expect(medium == PaneHeaderModel.Parts(directory: false, context: true))
    let narrow = PaneHeaderModel.parts(width: 200, for: named)
    #expect(narrow == PaneHeaderModel.Parts(directory: false, context: false))

    let drawn = header(width: 200, row: named).header
    #expect(drawn.shownKeys.isEmpty, "this pane has no chord to draw")
    #expect(!drawn.showsContext)
    #expect(!drawn.showsDirectory)
    #expect(drawn.shownName.contains("Qm storage"), "the name never goes")
  }

  @Test("the context bar is drawn when the agent has said how full it is")
  func theContextBar() {
    #expect(header(row: testHeaderRow("w1:p1", context: 40)).header.showsContext)
    #expect(!header(row: testHeaderRow("w1:p1")).header.showsContext)
    // Nobody can say how full the context of an agent on an unreachable machine is.
    #expect(
      !header(row: testHeaderRow("w1:p1", state: "unreachable", context: 40)).header.showsContext)
  }

  @Test("a header has the whole story on hover")
  func theTooltip() {
    let row = testHeaderRow("w1:p1", state: "blocked", since: 60, context: 40)
    #expect(header(row: row).header.toolTip == SidebarModel.details(of: row))
  }

  @Test("the header is drawn again only when its row says something different")
  func redrawsOnlyOnAChange() {
    let chrome = header(row: testHeaderRow("w1:p1", state: "working"))
    #expect(chrome.headerRedrawCount == 1)
    chrome.apply(header: testHeaderRow("w1:p1", state: "working"))
    #expect(chrome.headerRedrawCount == 1)
    chrome.apply(header: testHeaderRow("w1:p1", state: "blocked"))
    #expect(chrome.headerRedrawCount == 2)
  }

  /// What a window holds: a chrome per pane, and the joins that feed them.
  @MainActor
  private final class Panes {
    var chromes: [PaneKey: PaneChrome] = [:]
    var now = epoch
    let headers: PaneHeaders
    let panes: [Roster.Pane]

    init(count: Int) {
      panes = (1...count).map {
        Roster.Pane(
          key: PaneKey(daemon: "local", pane: "w1:p\($0)"), place: $0, press: $0,
          label: "p\($0)", onScreen: true, callsign: "otter\($0)", directory: "/src/p\($0)")
      }
      var chromes: [PaneKey: PaneChrome] = [:]
      for pane in panes {
        let chrome = PaneChrome(
          frame: NSRect(x: 0, y: 0, width: 400, height: 200),
          surface: SurfaceView(frame: .zero))
        chrome.attach(paneID: pane.key.pane)
        chromes[pane.key] = chrome
      }
      self.chromes = chromes
      headers = PaneHeaders { chromes[$0] }
      headers.clock = { [unowned self] in now }
    }

    var roster: Roster {
      Roster(
        tabs: [
          Roster.Tab(
            id: "w1:t1", daemons: ["local"], place: 1, label: "t", onScreen: true, panes: panes)
        ], machines: [])
    }

    var redraws: Int { chromes.values.reduce(0) { $0 + $1.headerRedrawCount } }

    func agents(lastIs state: String) -> [PaneKey: PaneAgent] {
      var agents: [PaneKey: PaneAgent] = [:]
      for pane in panes { agents[pane.key] = agent("idle", since: 90) }
      agents[panes[0].key] = agent("blocked", since: 90)
      if panes.count > 1 { agents[panes[1].key] = agent("done", since: 30) }
      if panes.count > 2 { agents[panes[panes.count - 1].key] = agent(state, since: 90) }
      return agents
    }

    private func agent(_ state: String, since seconds: Double) -> PaneAgent {
      var agent = PaneAgent(state: state)
      agent.sinceMS = Int64((now.timeIntervalSince1970 - seconds) * 1000)
      return agent
    }
  }

  @Test("what a change costs in redrawn headers, at one pane and at fifteen", arguments: [1, 15])
  func redrawCost(panes count: Int) {
    let window = Panes(count: count)
    let timed = min(count, 2)

    window.headers.apply(roster: window.roster, agents: window.agents(lastIs: "idle"))
    #expect(window.redraws == count, "the first draw is every header")
    #expect(window.headers.showsTime, "a blocked pane shows a time, so the minute is counted")

    // The same roster and states again, as a republish that changed nothing sends them.
    let before = window.redraws
    window.headers.apply(roster: window.roster, agents: window.agents(lastIs: "idle"))
    #expect(window.redraws == before, "an unchanged republish redrew headers")

    // One agent changes what it is doing: the one message that arrives all day.
    if count > 2 {
      let last = window.panes[count - 1].key
      window.headers.apply(pane: last, agent: PaneAgent(state: "working"))
      #expect(window.redraws == before + 1, "a state change redrew more than its own header")
    }

    // A minute passes: only the headers showing a time say something different, and a few
    // seconds more change nothing.
    let afterChange = window.redraws
    window.now = window.now.addingTimeInterval(60)
    window.headers.tick()
    #expect(window.redraws == afterChange + timed, "the minute redrew headers with no time")
    window.now = window.now.addingTimeInterval(10)
    window.headers.tick()
    #expect(window.redraws == afterChange + timed, "ten seconds redrew a header")
  }

  @Test("a header for a pane the roster does not hold is left empty")
  func anUnknownPaneHasNoHeader() {
    let window = Panes(count: 1)
    window.headers.apply(roster: Roster(tabs: []), agents: [:])
    #expect(window.chromes.values.allSatisfy { $0.headerRow == nil })
  }
}
