import AppKit
import Testing

@testable import MusterMac

// The Needs you strip: what the core's asking list becomes on screen, in the core's order, and what
// it costs to keep right. The ranking itself is the core's (`Attention::asking`) and is tested
// there; what is pinned here is that the shell never reorders it, says each card in words, and
// draws no more than changed.

private let epoch = Date(timeIntervalSince1970: 1_000_000)

@MainActor
private func pane(
  _ id: String, tabPress: Int = 0, press: Int = 0, callsign: String = "", subtitle: String = "",
  daemon: String = "local"
) -> Roster.Pane {
  Roster.Pane(
    key: PaneKey(daemon: daemon, pane: id), place: press, tabPress: tabPress, press: press,
    label: callsign.isEmpty ? id : "\(callsign) · src", subtitle: subtitle, onScreen: false,
    callsign: callsign)
}

@MainActor
private func roster(_ panes: [Roster.Pane], hidden: [Roster.Pane] = []) -> Roster {
  Roster(
    tabs: [
      Roster.Tab(
        id: "w1:t1", daemons: ["local"], place: 1, label: "t", onScreen: false, panes: panes)
    ], machines: [],
    hidden: hidden.isEmpty
      ? [] : [Roster.Tab(id: "w1:t9", place: 9, label: "h", onScreen: false, panes: hidden)])
}

/// The callsigns the core would name these panes by, as the entries carry them.
private let callsigns = ["w1:p1": "ivory", "w1:p2": "lynx", "w1:p3": "otter", "w1:p8": "walrus"]

/// What the core would carry for a blocked pane's subtitle, as the entries carry it.
private let subtitles = ["w1:p1": "Which dirs?"]

@MainActor
private func entry(
  _ alert: String, pane: String = "", group: String = "", since: Double? = nil,
  preview: String = "", callsign: String? = nil, label: String = "", subtitle: String? = nil
) -> AskingEntry {
  var raw = Muster_AskingEntry()
  raw.daemonID = "local"
  raw.paneID = pane
  raw.callsign = callsign ?? callsigns[pane] ?? ""
  raw.label = label
  raw.subtitle = subtitle ?? (alert == "blocked" ? subtitles[pane] ?? "" : "")
  raw.group = group
  raw.alert = alert
  raw.sinceMs = since.map { Int64((epoch.timeIntervalSince1970 - $0) * 1000) } ?? 0
  raw.preview = preview
  return AskingEntry(raw)
}

@MainActor
private func agent(_ state: String, since: Double? = nil) -> PaneAgent {
  var agent = PaneAgent(state: state)
  agent.sinceMS = since.map { Int64((epoch.timeIntervalSince1970 - $0) * 1000) } ?? 0
  return agent
}

@MainActor
private func cards(
  _ entries: [AskingEntry], roster: Roster, agents: [PaneKey: PaneAgent] = [:],
  groups: [SidebarModel.MessageGroup] = []
) -> [NeedsYouModel.Card] {
  NeedsYouModel.cards(
    entries: entries, roster: roster, agents: agents, groups: groups, now: epoch)
}

@Suite("the needs you strip")
@MainActor
struct NeedsYouTests {
  private let p1 = pane("w1:p1", tabPress: 1, press: 1, callsign: "ivory", subtitle: "Which dirs?")
  private let p2 = pane("w1:p2", tabPress: 2, press: 3, callsign: "lynx")
  private let p3 = pane("w1:p3", callsign: "otter")

  private var agents: [PaneKey: PaneAgent] {
    [
      p1.key: agent("blocked", since: 60), p2.key: agent("stuck", since: 240),
      p3.key: agent("done", since: 120),
    ]
  }

  @Test("the cards come in the core's order, each saying what it wants and for how long")
  func theCoreOrderIsKept() {
    let entries = [
      entry("blocked", pane: "w1:p1", since: 60), entry("stuck", pane: "w1:p2", since: 240),
      entry("message", group: "ui-council", preview: "Can you confirm?"),
      entry("done", pane: "w1:p3", since: 120),
      entry("notified", pane: "w1:p3", since: 5),
    ]
    let groups = [
      SidebarModel.MessageGroup(daemon: "local", group: "ui-council", unread: 3, toYou: 2)
    ]
    let made = cards(entries, roster: roster([p1, p2, p3]), agents: agents, groups: groups)

    #expect(made.map(\.title) == ["ivory", "lynx", "ui-council", "otter", "otter"])
    #expect(made.map(\.word) == ["Blocked 1m", "Stuck 4m", "2 to you", "Done 2m", "Notified <1m"])
    #expect(made.map(\.state) == ["blocked", "stuck", "", "done", "done"])
    #expect(made.map(\.isTinted) == [true, true, false, false, false])
  }

  @Test(
    "a blocked card carries its row's subtitle, and the others carry nothing but a message's start")
  func whatEachCardSaid() {
    let made = cards(
      [
        entry("blocked", pane: "w1:p1", since: 1), entry("stuck", pane: "w1:p2", since: 1),
        entry("message", group: "g", preview: "start of the latest message"),
      ], roster: roster([p1, p2]), agents: agents)
    #expect(made.map(\.detail) == ["Which dirs?", "", "start of the latest message"])
  }

  @Test(
    "a pane's card carries the keys that reach it, and a group's and a hidden pane's carry none")
  func theChord() {
    let hidden = pane("w1:p8", callsign: "walrus")
    let made = cards(
      [
        entry("blocked", pane: "w1:p1"), entry("stuck", pane: "w1:p2"),
        entry("message", group: "g"), entry("done", pane: "w1:p8"),
      ], roster: roster([p1, p2], hidden: [hidden]), agents: agents)
    #expect(made.map(\.keys) == [[1, 1], [2, 3], [], []])
    #expect(made[3].title == "walrus", "a hidden pane is still named")
  }

  @Test("a group says the messages for you, else those unread, else only that there is one")
  func theGroupWords() {
    func word(_ group: SidebarModel.MessageGroup?) -> String {
      cards(
        [entry("message", group: "g")], roster: roster([]), groups: group.map { [$0] } ?? []
      )[0].word
    }
    #expect(
      word(SidebarModel.MessageGroup(daemon: "local", group: "g", unread: 3, toYou: 2))
        == "2 to you")
    #expect(
      word(SidebarModel.MessageGroup(daemon: "local", group: "g", unread: 3, toYou: 0))
        == "3 unread")
    #expect(word(nil) == "Message")
  }

  @Test("a pane the core could not name still has a card, titled by its id")
  func anUnnamedPaneIsStillAsking() {
    let made = cards([entry("done", pane: "w9:p9", since: 60)], roster: roster([]))
    #expect(made.count == 1)
    #expect(made[0].title == "w9:p9")
    #expect(made[0].state == "unknown")
    #expect(made[0].word == "Done 1m")
  }

  @Test("a pane another window holds is titled by the name its entry carries, not its id")
  func anotherWindowsPaneIsNamed() {
    let named = entry("done", pane: "w9:p9", since: 60, callsign: "prairie", label: "prairie · src")
    let unlabeled = entry("done", pane: "w9:p8", since: 60, label: "src")
    let made = cards([named, unlabeled], roster: roster([]))
    #expect(made.map(\.title) == ["prairie", "src"], "the callsign, else the label")
  }

  @Test("a blocked pane another window holds shows the subtitle its entry carries")
  func anotherWindowsBlockedPaneHasItsDetail() {
    let blocked = entry(
      "blocked", pane: "w9:p9", since: 60, callsign: "prairie", subtitle: "Run the tests?")
    let done = entry("done", pane: "w9:p8", since: 60, callsign: "heron", subtitle: "left over")
    let made = cards([blocked, done], roster: roster([]))
    #expect(made.map(\.detail) == ["Run the tests?", ""], "only a blocked card draws it")
  }

  @Test("a blocked agent that is waiting on its own build says so, as its row does")
  func theRowsWordsWinOverTheSubtitle() {
    var waiting = PaneAgent(state: "waiting")
    waiting.waiting = "cargo build"
    var gone = PaneAgent(state: "unreachable")
    gone.lastState = "blocked"
    let agents = [
      PaneKey(daemon: "local", pane: "w9:p9"): waiting,
      PaneKey(daemon: "local", pane: "w9:p8"): gone,
    ]
    let made = cards(
      [
        entry("blocked", pane: "w9:p9", callsign: "prairie", subtitle: "Run the tests?"),
        entry("blocked", pane: "w9:p8", callsign: "heron", subtitle: "Run the tests?"),
      ], roster: roster([]), agents: agents)
    #expect(made.map(\.detail) == ["waiting on cargo build", "unreachable · was blocked"])
  }

  @Test("a renamed pane is drawn under the name its next list carries")
  func aRenameArrivesWithTheList() {
    let before = cards(
      [entry("done", pane: "w1:p3", since: 60, callsign: "otter", label: "otter · a")],
      roster: roster([p3]), agents: agents)
    let after = cards(
      [entry("done", pane: "w1:p3", since: 60, callsign: "heron", label: "heron · a")],
      roster: roster([p3]), agents: agents)
    #expect(before[0].title == "otter")
    #expect(after[0].title == "heron")
  }

  @Test("only a card showing a time changes with the minute")
  func timedCards() {
    let made = cards(
      [entry("blocked", pane: "w1:p1", since: 60), entry("message", group: "g")],
      roster: roster([p1]), agents: agents)
    #expect(made.map(\.isTimed) == [true, false])
    #expect(NeedsYouModel.showsTime(made))
    #expect(!NeedsYouModel.showsTime([made[1]]))
    // No `since` is no time, rather than a time since 1970.
    let unknown = cards([entry("blocked", pane: "w1:p1")], roster: roster([p1]), agents: agents)
    #expect(unknown[0].word == "Blocked" && !unknown[0].isTimed)
  }

  // MARK: The view

  private func sidebar(
    height: CGFloat = 800, entries: [AskingEntry], roster: Roster, agents: [PaneKey: PaneAgent],
    groups: [SidebarModel.MessageGroup] = []
  ) -> SidebarView {
    let sidebar = SidebarView(frame: NSRect(x: 0, y: 0, width: SidebarModel.width, height: height))
    sidebar.clock = { epoch }
    sidebar.groups = groups
    sidebar.apply(roster: roster, agents: agents)
    sidebar.asking = entries
    sidebar.layoutSubtreeIfNeeded()
    return sidebar
  }

  @Test("the strip is absent while nothing asks, and is there with a card for each that does")
  func absentUntilSomethingAsks() {
    let list = sidebar(entries: [], roster: roster([p1]), agents: agents)
    #expect(!list.needsYou.isPresent)
    #expect(list.needsYou.frame.height == 0)

    list.asking = [entry("blocked", pane: "w1:p1", since: 60), entry("done", pane: "w1:p3")]
    list.layoutSubtreeIfNeeded()
    #expect(list.needsYou.isPresent)
    #expect(list.needsYou.shownCards.count == 2)
    #expect(list.needsYou.shownCount == 2)
    #expect(list.needsYou.frame.height > NeedsYouStripView.headerHeight)
    #expect(list.needsYou.frame.maxY == list.bounds.maxY, "across the top")

    list.asking = []
    list.layoutSubtreeIfNeeded()
    #expect(list.needsYou.frame.height == 0, "gone again when nobody asks")
  }

  @Test("a card draws its icon, its words and its keys, and a blocked one is tinted")
  func aCardDrawsWhatItSays() throws {
    let list = sidebar(
      entries: [
        entry("blocked", pane: "w1:p1", since: 60), entry("done", pane: "w1:p3", since: 120),
      ],
      roster: roster([p1, p3]), agents: agents)
    let blocked = try #require(list.needsYou.shownCards.first)
    #expect(blocked.shownIcon == "blocked")
    #expect(blocked.shownTitle == "ivory")
    #expect(blocked.shownWord == "Blocked 1m")
    #expect(blocked.shownDetail == "Which dirs?")
    #expect(blocked.shownKeys == [1, 1])
    #expect(blocked.showsTint)

    let done = list.needsYou.shownCards[1]
    #expect(done.shownIcon == "done")
    #expect(done.shownDetail == nil)
    #expect(!done.showsTint)
  }

  @Test("a group card draws no state icon and its message's start")
  func aGroupCard() throws {
    let list = sidebar(
      entries: [entry("message", group: "ui-council", preview: "Can you confirm?")],
      roster: roster([]), agents: [:],
      groups: [SidebarModel.MessageGroup(daemon: "local", group: "ui-council", unread: 2, toYou: 2)]
    )
    let card = try #require(list.needsYou.shownCards.first)
    #expect(card.shownIcon == nil)
    #expect(card.shownTitle == "ui-council")
    #expect(card.shownWord == "2 to you")
    #expect(card.shownDetail == "Can you confirm?")
    #expect(card.shownKeys.isEmpty)
  }

  @Test("a click on a card goes to its pane, or opens its group's conversation")
  func clicksRoute() throws {
    let list = sidebar(
      entries: [entry("blocked", pane: "w1:p1"), entry("message", group: "g")],
      roster: roster([p1]), agents: agents)
    var panes: [PaneKey] = []
    var groups: [String] = []
    list.onPanePicked = { panes.append($0) }
    list.onGroupPicked = { groups.append("\($0)/\($1)") }

    list.needsYou.shownCards[0].press()
    list.needsYou.shownCards[1].press()

    #expect(panes == [p1.key])
    #expect(groups == ["local/g"])
  }

  @Test("the header asks to fold an open strip and to open a folded one")
  func theFoldIsARequest() {
    let list = sidebar(
      entries: [entry("done", pane: "w1:p3")], roster: roster([p3]), agents: agents)
    var asked: [Bool] = []
    list.onNeedsYouToggled = { asked.append($0) }

    list.needsYou.pressHeader()
    list.needsYouFolded = true
    list.needsYou.pressHeader()

    #expect(asked == [true, false])
  }

  @Test("folded, the strip is one line of the icons of whoever is asking, in the core's order")
  func foldedIsOneLine() {
    let list = sidebar(
      entries: [
        entry("blocked", pane: "w1:p1"), entry("message", group: "g"), entry("done", pane: "w1:p3"),
      ], roster: roster([p1, p3]), agents: agents)
    list.needsYouFolded = true
    list.layoutSubtreeIfNeeded()

    #expect(list.needsYou.frame.height == NeedsYouStripView.headerHeight)
    #expect(list.needsYou.shownHeaderIcons == ["blocked", "message", "done"])
    #expect(list.needsYou.shownCount == 3)
  }

  @Test("a long list of askers is held to a share of the sidebar")
  func theStripLeavesRoomForTheList() {
    let panes = (1...15).map { pane("w1:p\($0)", callsign: "a\($0)", subtitle: "Which?") }
    var agents: [PaneKey: PaneAgent] = [:]
    for pane in panes { agents[pane.key] = agent("blocked", since: 60) }
    let list = sidebar(
      height: 500, entries: panes.map { entry("blocked", pane: $0.key.pane, since: 60) },
      roster: roster(panes), agents: agents)

    let share = (500 * SidebarView.stripShare).rounded(.down)
    #expect(list.needsYou.frame.height <= share)
    #expect(list.needsYou.frame.height > NeedsYouStripView.headerHeight)
  }

  @Test("a saved change of colors repaints the cards, though nothing they say has changed")
  func colorsReachTheStrip() throws {
    defer { PaneAppearance.adopt(chrome: .none) }
    PaneAppearance.adopt(chrome: .none)
    let list = sidebar(
      entries: [entry("blocked", pane: "w1:p1")], roster: roster([p1]), agents: agents)
    let before = try #require(list.needsYou.shownCards.first?.edgeColor)

    PaneAppearance.adopt(
      chrome: Core.Chrome(
        divider: nil, focusRing: nil, agents: Core.AgentColors(blocked: "#00ff00")))
    list.reloadColors()

    let after = try #require(list.needsYou.shownCards.first?.edgeColor)
    #expect(before != after, "the card kept the color it was drawn with")
    #expect(after == NSColor(hex: "#00ff00"))
  }

  @Test("a time is on the clock while a card shows one, even if no row does")
  func theStripKeepsTheMinute() {
    let idle = [p3.key: agent("idle")]
    let list = sidebar(
      entries: [entry("notified", pane: "w1:p3", since: 5)], roster: roster([p3]), agents: idle)
    #expect(list.showsTime)
    list.asking = []
    #expect(!list.showsTime)
  }

  @Test(
    "what a change costs the strip in redrawn cards, at one pane and at fifteen",
    arguments: [1, 15])
  func redrawCost(panes count: Int) {
    let panes = (1...count).map { pane("w1:p\($0)", callsign: "a\($0)") }
    var agents: [PaneKey: PaneAgent] = [:]
    for pane in panes { agents[pane.key] = agent("blocked", since: 90) }
    func entries(preview: String) -> [AskingEntry] {
      panes.map { entry("blocked", pane: $0.key.pane, since: 90) }
        + [entry("message", group: "g", preview: preview)]
    }
    let list = SidebarView(frame: NSRect(x: 0, y: 0, width: SidebarModel.width, height: 800))
    var now = epoch
    list.clock = { now }
    list.apply(roster: roster(panes), agents: agents)
    list.asking = entries(preview: "one")
    #expect(list.stripRedrawCount == count + 1, "the first draw is every card")

    let before = list.stripRedrawCount
    list.apply(roster: roster(panes), agents: agents)
    list.asking = entries(preview: "one")
    #expect(list.stripRedrawCount == before, "an unchanged republish redrew cards")

    list.asking = entries(preview: "two")
    #expect(list.stripRedrawCount == before + 1, "a second message redrew more than its card")

    let afterChange = list.stripRedrawCount
    now = epoch.addingTimeInterval(60)
    list.tick()
    #expect(list.stripRedrawCount == afterChange + count, "the minute redrew a card with no time")
    now = now.addingTimeInterval(10)
    list.tick()
    #expect(list.stripRedrawCount == afterChange + count, "ten seconds redrew a card")
  }
}
