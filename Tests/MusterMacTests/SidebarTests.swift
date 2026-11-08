import AppKit
import Testing

@testable import MusterMac

// The list is where the founding desideratum stops depending on what happens to be on
// screen. Its decisions are all joins and orderings, which is exactly the kind of thing that
// looks right in a screenshot and is wrong for one daemon out of two.

@Suite("the sidebar lists what exists")
struct SidebarTests {
  @Test("the roster keeps the core's tab fold")
  func readsTabFold() {
    var tab = Muster_RosterTab()
    tab.tabID = "t1"
    tab.folded = true
    #expect(Roster.Tab(tab).folded)
  }

  @Test("the asking list keeps the core's entry")
  func readsAskingEntry() {
    var entry = Muster_AskingEntry()
    entry.daemonID = "local"
    entry.group = "g"
    entry.alert = "message"
    entry.sinceMs = 42
    entry.preview = "hello"
    let read = AskingEntry(entry)
    #expect(read.daemon == "local")
    #expect(read.group == "g")
    #expect(read.alert == "message")
    #expect(read.sinceMs == 42)
    #expect(read.preview == "hello")
  }

  /// One pane, numbered as the core would have numbered it.
  ///
  /// The place is stated rather than counted here, for the same reason a tab's is: the core
  /// decides it, and a helper that numbered for itself would make these tests agree with a rule
  /// the shell does not follow. Defaults to one, since most cases here are about something else.
  ///
  /// The chord that reaches it defaults to one press, its place, which is what the core sends in
  /// a window of one tab. A case about a window of several states both presses instead.
  private func pane(
    _ daemon: String, _ id: String, place: Int = 1, tabPress: Int = 0, press: Int? = nil,
    label: String? = nil, subtitle: String = "", givenName: String = "", onScreen: Bool = false,
    directory: String = ""
  ) -> Roster.Pane {
    Roster.Pane(
      key: PaneKey(daemon: daemon, pane: id), place: place, tabPress: tabPress,
      press: press ?? place, label: label ?? id, subtitle: subtitle, givenName: givenName,
      onScreen: onScreen, directory: directory)
  }

  /// One tab, numbered as the core would have numbered it.
  ///
  /// The place is stated rather than counted here, because the core decides it - a helper that
  /// numbered for itself would make these tests agree with a rule the shell does not follow.
  ///
  /// No press reaches it unless a case says one does, which is the only tab in a window.
  private func tab(
    _ daemon: String, _ id: String = "w1:t1", place: Int = 1, press: Int = 0,
    armed: Bool = false, label: String? = nil, onScreen: Bool = false, panes: [Roster.Pane]
  ) -> Roster.Tab {
    Roster.Tab(
      id: id, daemons: [daemon], place: place, press: press, armed: armed, label: label ?? id,
      onScreen: onScreen, panes: panes)
  }

  /// The color the core draws for the two machine names these cases use.
  private func color(of daemon: String) -> String {
    daemon == "local" ? "#e5484d" : "#8d9440"
  }

  /// A roster of tabs, with the machines behind them worked out from the panes.
  ///
  /// Derived rather than stated, because what the machines list decides up here is whether a
  /// pane row says which machine it is on - and that follows from how many are attached, which
  /// is exactly what the panes say. A case about a machine holding nothing states them instead.
  private func roster(
    _ tabs: [Roster.Tab], machines: [Roster.Machine]? = nil,
    numbering: Roster.Numbering = .panes
  ) -> Roster {
    var found: [String] = []
    for daemon in tabs.flatMap(\.panes).map(\.key.daemon) where !found.contains(daemon) {
      found.append(daemon)
    }
    return Roster(
      tabs: tabs,
      machines: machines
        ?? found.map { Roster.Machine(id: $0, state: "connected", panes: 1, color: color(of: $0)) },
      numbering: numbering)
  }

  @Test("panes from two machines sit in one flat list of tabs")
  func daemonsAreGrouped() {
    let roster = roster([
      tab("local", panes: [pane("local", "w1:p1")]),
      tab("devenv", place: 2, panes: [pane("devenv", "w1:p1")]),
    ])
    let rows = SidebarModel.rows(roster: roster, agents: [:])

    // No heading per machine: a tab may hold panes on two, so grouping by machine would be a
    // list that no longer describes the window beside it (MIP-2).
    #expect(rows.filter(\.isHeader).isEmpty)
    // The core's order, not re-sorted here. A list that sorted for itself would disagree
    // with the window it sits beside for any arrangement but the alphabetical one.
    #expect(rows.compactMap(\.pane?.daemon) == ["local", "devenv"])
    // And with two machines attached, every pane row says which it is on, in the color the core
    // gave that machine. With one it says nothing, because the answer would be the same on
    // every row.
    #expect(
      rows.compactMap(\.machine) == [
        SidebarModel.MachineMark(daemon: "local", color: "#e5484d"),
        SidebarModel.MachineMark(daemon: "devenv", color: "#8d9440"),
      ])
  }

  @Test("one machine marks nothing, and a heading carries its machine's mark")
  func theMachineMarkIsTheListsOwnLegend() {
    let alone = SidebarModel.rows(
      roster: roster([tab("local", panes: [pane("local", "w1:p1")])]), agents: [:])
    #expect(alone.allSatisfy { $0.machine == nil })

    // Two machines, one of them holding nothing, so it gets a heading. The heading's mark is the
    // one its panes would carry, which is what lets somebody learn the colors from the list.
    let rows = SidebarModel.rows(
      roster: roster(
        [tab("local", panes: [pane("local", "w1:p1")])],
        machines: [
          Roster.Machine(id: "local", state: "connected", panes: 1, color: "#e5484d"),
          Roster.Machine(id: "devenv", state: "disconnected", panes: 0, color: "#8d9440"),
        ]),
      agents: [:])
    let heading = try? #require(rows.first { $0.isMachine })
    #expect(heading?.machine == SidebarModel.MachineMark(daemon: "devenv", color: "#8d9440"))
    let paneRow = try? #require(rows.first { $0.isPane })
    #expect(paneRow?.machine == SidebarModel.MachineMark(daemon: "local", color: "#e5484d"))
    // A color says which machine to somebody who has learned the colors; hovering says it to
    // everybody else, and VoiceOver reads the same text.
    #expect(paneRow.map(SidebarModel.details(of:))?.contains("on local") == true)
  }

  /// What the sidebar's partial redraw rests on.
  ///
  /// One agent transition is the most frequent thing that happens in a window full of them, and
  /// the list redraws only the rows that came out different rather than reloading the table -
  /// which is what keeps the shell from undoing the core's care to make an agent-state change
  /// cost that change rather than a walk of every pane. That optimisation is only correct while
  /// a row is a function of its own subject: the moment one carries something window-wide - a
  /// total, a "how many are working" - every row differs on every blink and the diff quietly
  /// stops saving anything while still looking right.
  @Test("one agent changing state changes one row and leaves the rest identical")
  func aStateChangeTouchesOneRow() {
    let roster = roster([
      tab(
        "local",
        panes: [
          pane("local", "w1:p1", place: 1), pane("local", "w1:p2", place: 2),
          pane("local", "w1:p3", place: 3),
        ])
    ])
    let before = SidebarModel.rows(roster: roster, agents: [:])
    let after = SidebarModel.rows(
      roster: roster, agents: [PaneKey(daemon: "local", pane: "w1:p2"): "working"])

    #expect(before.count == after.count)
    let moved = before.indices.filter { before[$0] != after[$0] }
    #expect(moved.count == 1)
    #expect(after[moved[0]].pane == PaneKey(daemon: "local", pane: "w1:p2"))
  }

  @Test("two daemons handing out one pane id are two rows")
  func paneIdsAreScopedToTheirDaemon() {
    // The whole reason a row is keyed by the pair. Collapsing these would put one machine's
    // agent state on the other's row, and clicking it would open the wrong pane.
    let roster = roster([
      tab("local", panes: [pane("local", "w1:p1")]),
      tab("devenv", place: 2, panes: [pane("devenv", "w1:p1")]),
    ])
    let rows = SidebarModel.rows(
      roster: roster,
      agents: [
        PaneKey(daemon: "local", pane: "w1:p1"): "working",
        PaneKey(daemon: "devenv", pane: "w1:p1"): "blocked",
      ])

    let panes = rows.filter { $0.isPane }
    #expect(panes.count == 2)
    #expect(panes.map(\.state) == ["working", "blocked"])
  }

  @Test("a pane the core has said nothing about is unknown, not idle")
  func silenceIsNotSuccess() {
    // The roster and the states are two messages and arrive in either order, so a list built
    // from the first alone has no state for anything. Reading that as idle would paint a
    // window full of running agents as a window with nothing to do.
    let roster = roster([
      tab("local", panes: [pane("local", "w1:p1")])
    ])
    let rows = SidebarModel.rows(roster: roster, agents: [:])

    #expect(rows.last?.state == "unknown")
    #expect(
      PaneAppearance.defaultBorderColor(state: "unknown")
        != PaneAppearance.defaultBorderColor(state: "idle"))
  }

  @MainActor
  @Test("the dot agrees with the border the same state draws on a pane")
  func theListAgreesWithTheWindow() {
    // Two places show one state. A sidebar that picked its own palette would make a user
    // check both and trust neither. That now has to survive a repainted state too: the
    // colours are a person's to change, and a file that moved the border and not the dot
    // would be the same failure arriving by a new route.
    defer { PaneAppearance.adopt(chrome: .none) }
    let repainted = Core.Chrome(
      divider: nil, focusRing: nil,
      agents: Core.AgentColors(working: "#7aa2f7", blocked: "#ff9e64"))
    for chrome in [Core.Chrome.none, repainted] {
      PaneAppearance.adopt(chrome: chrome)
      for state in ["working", "blocked", "done", "idle", "unknown"] {
        #expect(SidebarModel.dotColor(state: state) == PaneAppearance.borderColor(state: state))
      }
    }
  }

  @Test("a row says whether anything is showing its pane")
  func offScreenPanesAreMarked() {
    let roster = roster([
      tab(
        "local",
        panes: [
          pane("local", "w1:p1", onScreen: true), pane("local", "w1:p9", onScreen: false),
        ])
    ])
    let rows = SidebarModel.rows(roster: roster, agents: [:]).filter { $0.isPane }

    #expect(rows.map(\.onScreen) == [true, false])
  }

  @Test("an empty roster draws nothing, not an empty heading")
  func nothingIsNothing() {
    // A window on the way up has an attached daemon and no panes yet. A heading over no rows
    // reads as a machine that lost its session.
    #expect(SidebarModel.rows(roster: roster([]), agents: [:]).isEmpty)
    let bare = roster([])
    #expect(SidebarModel.rows(roster: bare, agents: [:]).isEmpty)
  }

  @Test("a window with one tab draws no caption for it")
  func oneTabIsNotWorthALevel() {
    // The common case, and the reason this rule exists: with nothing to navigate between, a
    // row saying which tab you are in answers a question nobody has, and it costs a level of
    // indentation off every label in a 200pt column.
    let roster = roster([
      tab("local", onScreen: true, panes: [pane("local", "w1:p1")])
    ])
    let rows = SidebarModel.rows(roster: roster, agents: [:])

    #expect(rows.map(\.kind) == [.pane(tabPress: 0, press: 1)])
  }

  @Test("a second tab anywhere in the window gives every tab a caption")
  func tabsAppearTogetherOrNotAtAll() {
    // Including the tabs on a daemon that only holds one. Captions in patches would read as a
    // boundary that comes and goes, which is worse than one that is always there.
    let roster = roster([
      tab(
        "local", "w1:t1", place: 1, label: "one", onScreen: true,
        panes: [pane("local", "w1:p1", place: 1)]),
      tab(
        "local", "w1:t2", place: 2, label: "two",
        panes: [pane("local", "w1:p2", place: 2)]),
      tab(
        "devenv", "w1:t1", place: 3, label: "three",
        panes: [pane("devenv", "w1:p1", place: 3)]),
    ])
    let rows = SidebarModel.rows(roster: roster, agents: [:])

    let kinds: [SidebarModel.Kind] = [
      .tab(press: 0), .pane(tabPress: 0, press: 1), .tab(press: 0), .pane(tabPress: 0, press: 2),
      .tab(press: 0), .pane(tabPress: 0, press: 3),
    ]
    #expect(rows.map(\.kind) == kinds)
    #expect(rows.filter { $0.isTab }.map(\.label) == ["one", "two", "three"])
  }

  /// The window both cases below are about, at rest and then one press in.
  ///
  /// Stated once because the two cases have to be the *same* window a keystroke apart - that is
  /// what makes "nothing moved" an assertion rather than two lists that happen to agree. The
  /// first tab holds one pane, so it does not arm and its pane's chord is the one press onto
  /// the tab: ⌘1 ⌘1 would be two tab jumps rather than that pane.
  private func aWindowOfTwoTabs(armed: Bool) -> Roster {
    roster(
      [
        tab(
          "local", "w1:t1", place: 1, press: 1, onScreen: !armed,
          panes: [pane("local", "w1:p1", place: 1, tabPress: 1, press: 0)]),
        tab(
          "local", "w1:t2", place: 2, press: 2, armed: armed, onScreen: armed,
          panes: [
            pane("local", "w1:p2", place: 2, tabPress: 2, press: 1),
            pane("local", "w1:p3", place: 3, tabPress: 2, press: 2),
          ]),
      ], numbering: armed ? .panesInTab : .tabs)
  }

  /// What every row of that window carries, which is the same before and after a press.
  private var twoTabChords: [SidebarModel.Kind] {
    [
      .tab(press: 1), .pane(tabPress: 1, press: 0),
      .tab(press: 2), .pane(tabPress: 2, press: 1), .pane(tabPress: 2, press: 2),
    ]
  }

  @Test("every row carries the whole chord that reaches it")
  func everyRowDrawsItsWholeChord() {
    // What a window of two tabs looks like before anything is pressed. Every
    // pane says its own address, including the ones in the tab nothing is showing - which is
    // the case the change was for: knowing what to press to reach an agent used to cost a
    // press and a second look (kan a_2LSUoy7dd).
    let rows = SidebarModel.rows(roster: aWindowOfTwoTabs(armed: false), agents: [:])

    #expect(rows.map(\.kind) == twoTabChords)
    // Two columns on a pane row and one on a caption, whether or not the row fills them. The
    // rows that carry nothing are scattered through a real list - a pane past the ninth in its
    // tab, a tab past the ninth - and labels lining up only where a chord happened to exist
    // would read as a column with holes in it.
    #expect(rows.filter { $0.isPane }.allSatisfy { $0.reservedPresses == 2 })
    #expect(rows.filter { $0.isTab }.allSatisfy { $0.reservedPresses == 1 })
    // Nothing is half-typed, so every press is a reference rather than a live keystroke.
    #expect(rows.contains { $0.isSecondPress } == false)
  }

  @Test("a press names a tab without moving a single number in the list")
  func aPressMarksTheArmedTabInsteadOfMoving() {
    // The same window one keystroke later. The numbers used to move here - onto the named
    // tab's panes and off everything else - which is what left a pane in an unfocused tab with
    // no readable address and made a digit mean different things depending on what you had
    // already pressed. What ⌘2 changes now is which presses are live.
    let rows = SidebarModel.rows(roster: aWindowOfTwoTabs(armed: true), agents: [:])

    #expect(rows.map(\.kind) == twoTabChords)
    #expect(rows.filter { $0.isPane }.allSatisfy { $0.reservedPresses == 2 })

    // The mark is the whole of the replacement: exactly the named tab's panes say that their
    // second press is the keystroke about to be made, and nothing else in the list does. A
    // caption never can - its own press is a first press or none.
    let live = rows.filter(\.isSecondPress)
    #expect(
      live.map(\.pane) == [
        PaneKey(daemon: "local", pane: "w1:p2"), PaneKey(daemon: "local", pane: "w1:p3"),
      ])
  }

  @Test("a numbered tab is drawn even in a window that would hide its caption")
  func oneTabStillShowsACaptionWhenItIsNumbered() {
    // A press nothing draws is a chord nobody can find, so the rule that a single tab needs
    // no caption gives way to a tab that carries one. The core no longer numbers the tab in a
    // window that holds only one - such a window numbers panes, because with one tab a pane's
    // place in the window and in its tab are the same number - so this pins the guard rather
    // than a state a running window reaches. What reaches a row is the core's answer, and the
    // list has to draw whatever it is handed.
    let roster = roster([
      tab(
        "local", "w1:t1", place: 1, press: 1, onScreen: true,
        panes: [pane("local", "w1:p1", place: 1, press: 0)])
    ])

    let rows = SidebarModel.rows(roster: roster, agents: [:])

    #expect(rows.map(\.kind) == [.tab(press: 1), .pane(tabPress: 0, press: 0)])
  }

  @Test("a row can be dropped on a pane row on either machine, and on a caption")
  func aDropMayCrossMachines() throws {
    let roster = roster([
      tab(
        "local", "w1:t1", place: 1, label: "here", onScreen: true,
        panes: [pane("local", "w1:p1", place: 1), pane("local", "w1:p2", place: 2)]),
      tab(
        "devenv", "w2:t1", place: 2, label: "there",
        panes: [pane("devenv", "w2:p1", place: 3)]),
    ])
    let rows = SidebarModel.rows(roster: roster, agents: [:])
    let dragged = PaneKey(daemon: "local", pane: "w1:p1")

    let sameDaemon = try #require(rows.first { $0.pane == PaneKey(daemon: "local", pane: "w1:p2") })
    let otherDaemon = try #require(
      rows.first { $0.pane == PaneKey(daemon: "devenv", pane: "w2:p1") })
    let caption = try #require(rows.first { $0.isTab })

    #expect(SidebarModel.canArrange(dragged, onto: sameDaemon))
    // Beside another machine's pane is a place in a tab's tree like any other (MIP-8).
    #expect(SidebarModel.canArrange(dragged, onto: otherDaemon))
    #expect(SidebarModel.canArrange(dragged, onto: caption))
  }

  @Test("dropping a row on itself is allowed and means nothing")
  func aDropOnItselfIsNotAnError() throws {
    // An accidental drag, which is a mistake with no cost. Refusing it would put a cursor
    // change and a log line in front of somebody who twitched, and the core answers it by
    // doing nothing.
    let roster = roster([
      tab("local", panes: [pane("local", "w1:p1", place: 1)])
    ])
    let rows = SidebarModel.rows(roster: roster, agents: [:])
    let itself = PaneKey(daemon: "local", pane: "w1:p1")

    let row = try #require(rows.first { $0.isPane })

    #expect(SidebarModel.canArrange(itself, onto: row))
  }

  @Test("the number on a row is the pane's place, counting across the whole window")
  func paneNumbersCrossDaemons() {
    // The chord's half of the same fact the caption test above covers for tabs: ⌘3 is the third
    // numbered row down the sidebar, whichever machine holds it. The number comes from the core
    // rather than from the row's position here, so a shell that renumbered for itself would
    // show one number and the keyboard would mean another.
    let roster = roster([
      tab(
        "local", "w1:t1", place: 1, onScreen: true,
        panes: [pane("local", "w1:p1", place: 1), pane("local", "w1:p2", place: 2)]),
      tab("devenv", "w1:t1", place: 2, panes: [pane("devenv", "w1:p1", place: 3)]),
    ])

    let rows = SidebarModel.rows(roster: roster, agents: [:])

    #expect(
      rows.filter { $0.isPane }.map(\.kind)
        == [
          .pane(tabPress: 0, press: 1), .pane(tabPress: 0, press: 2), .pane(tabPress: 0, press: 3),
        ])
  }

  @Test("a tab caption says whether a region is showing it")
  func theTabOnScreenIsMarked() {
    // Which tab you are looking at is a different question from which pane you are typing
    // into, and in a two-region window they are two different tabs. The caption answers the
    // first; the keyboard highlight answers the second.
    let roster = roster([
      tab("local", "w1:t1", place: 1, onScreen: true, panes: [pane("local", "w1:p1")]),
      tab("local", "w1:t2", place: 2, onScreen: false, panes: [pane("local", "w1:p2")]),
    ])
    let rows = SidebarModel.rows(roster: roster, agents: [:]).filter {
      if case .tab = $0.kind { return true }
      return false
    }

    #expect(rows.map(\.onScreen) == [true, false])
  }

  @Test("a pane can be dropped on a caption from any machine, and on a machine's row from none")
  func aDropOnACaptionMayCrossMachines() throws {
    // What MIP-2 stage four is, as a gesture: dragging a devenv agent onto a laptop tab's
    // caption puts it in that tab. The pane stays on the devenv - it is a process - and what
    // changes is which tab it belongs to, which is why this one drop may cross machines where a
    // drop onto a pane row may not.
    let roster = roster(
      [
        tab(
          "local", "w1:t1", place: 1, label: "here", onScreen: true,
          panes: [pane("local", "w1:p1", place: 1)]),
        tab(
          "devenv", "w2:t1", place: 2, label: "there", panes: [pane("devenv", "w2:p1", place: 2)]),
      ],
      machines: [
        Roster.Machine(id: "local", state: "connected", panes: 1),
        Roster.Machine(id: "devenv", state: "connected", panes: 0),
      ])
    let rows = SidebarModel.rows(roster: roster, agents: [:])
    let onDevenv = PaneKey(daemon: "devenv", pane: "w2:p1")

    let laptopCaption = try #require(rows.first { $0.isTab && $0.tab == "w1:t1" })
    #expect(SidebarModel.canArrange(onDevenv, onto: laptopCaption))

    // A machine's row names no tab, so there is nothing to ask for.
    let machine = try #require(rows.first { $0.isMachine })
    #expect(!SidebarModel.canArrange(onDevenv, onto: machine))
  }

  @Test("the human's groups sit under a caption after the tabs, with what waits in each")
  func theHumansGroupsAreListedLast() {
    let groups = [
      SidebarModel.MessageGroup(daemon: "local", group: "review", unread: 3, toYou: 1),
      SidebarModel.MessageGroup(daemon: "local", group: "standup", unread: 0, toYou: 0),
    ]
    let rows = SidebarModel.rows(
      roster: roster([tab("local", panes: [pane("local", "w1:p1")])]), agents: [:],
      groups: groups)
    #expect(rows.map(\.label) == ["w1:p1", "Messages", "review", "standup"])
    let caption = rows[1]
    #expect(caption.isHeader && !caption.isDestination, "the caption goes nowhere")
    let review = rows[2]
    #expect(review.isGroup && review.isDestination && review.daemon == "local")
    #expect(SidebarModel.accessories(of: review) == [.toYou(1), .unread(3)])
    #expect(SidebarModel.accessories(of: rows[3]) == [], "a group read to the end is quiet")
    // One machine says nothing about which; the pane rows follow the same rule.
    #expect(review.machine == nil)
    // A group can go nowhere a pane can be dragged.
    #expect(!SidebarModel.canArrange(PaneKey(daemon: "local", pane: "w1:p1"), onto: review))

    let none = SidebarModel.rows(
      roster: roster([tab("local", panes: [pane("local", "w1:p1")])]), agents: [:])
    #expect(!none.contains { $0.label == "Messages" }, "no caption over no groups")
  }

  /// A hidden pane is out of every window, so it is listed apart and numbered by nothing; picking
  /// its row goes to it, which brings it back.
  @Test("hidden panes are listed under a caption of their own, after the window's tabs")
  func hiddenPanesAreListedApart() {
    let hiddenTab = tab(
      "local", "w1:t9", place: 0, panes: [pane("local", "w1:p9", place: 0, press: 0)])
    let base = roster([tab("local", panes: [pane("local", "w1:p1")])])
    let roster = Roster(
      tabs: base.tabs, machines: base.machines, numbering: base.numbering, hidden: [hiddenTab])
    let groups = [SidebarModel.MessageGroup(daemon: "local", group: "review", unread: 0, toYou: 0)]
    let rows = SidebarModel.rows(roster: roster, agents: [:], groups: groups)

    #expect(rows.map(\.label) == ["w1:p1", "Hidden", "w1:p9", "Messages", "review"])
    let caption = rows[1]
    #expect(caption.kind == .hidden(panes: 1, urgent: 0, collapsed: false))
    #expect(caption.isHiddenCaption && caption.isHeader && !caption.isDestination)
    #expect(caption.state.isEmpty, "an open caption's rows carry their own states")
    #expect(SidebarModel.accessories(of: caption) == [.disclosure(collapsed: false)])
    let hidden = rows[2]
    #expect(hidden.hidden && hidden.isPane && hidden.isDestination)
    #expect(hidden.kind == .pane(tabPress: 0, press: 0), "no chord reaches a hidden pane")
    #expect(!rows[0].hidden)
    #expect(roster.panes.map(\.key.pane) == ["w1:p1"], "a hidden pane is drawn by nothing")
    #expect(
      roster.pane(PaneKey(daemon: "local", pane: "w1:p9"))?.key.pane == "w1:p9",
      "a hidden pane's entry is found, so renaming its row starts from its name")
    #expect(SidebarModel.draggedTab(of: hidden, in: rows) == nil)
    #expect(!SidebarModel.canArrange(PaneKey(daemon: "local", pane: "w1:p1"), onto: hidden))

    let none = SidebarModel.rows(roster: base, agents: [:])
    #expect(!none.contains { $0.isHiddenCaption }, "no caption over no hidden panes")
  }

  /// Folded, the caption stands in for its panes: it says how many there are and paints the
  /// most urgent state among them, so a hidden agent that is blocked is not lost from the list.
  @Test("a folded Hidden caption draws no panes, and says how many and the most urgent state")
  func aFoldedHiddenCaptionSummarizesItsPanes() {
    let ids = ["w1:p7", "w1:p8", "w1:p9"]
    let hiddenTab = tab(
      "local", "w1:t9", place: 0, panes: ids.map { pane("local", $0, place: 0, press: 0) })
    let base = roster([tab("local", panes: [pane("local", "w1:p1")])])
    let roster = Roster(
      tabs: base.tabs, machines: base.machines, numbering: base.numbering, hidden: [hiddenTab])
    let key = { PaneKey(daemon: "local", pane: $0) }
    let agents: [PaneKey: PaneAgent] = [
      key("w1:p7"): "done", key("w1:p8"): "blocked", key("w1:p9"): "working",
    ]

    let rows = SidebarModel.rows(roster: roster, agents: agents, hiddenCollapsed: true)
    #expect(rows.map(\.label) == ["w1:p1", "Hidden"], "a folded caption still drew its panes")
    let caption = rows[1]
    #expect(caption.kind == .hidden(panes: 3, urgent: 1, collapsed: true))
    #expect(caption.state == "blocked", "a blocked hidden agent was lost behind the fold")
    #expect(
      SidebarModel.accessories(of: caption) == [.hiddenPanes(3), .disclosure(collapsed: true)])
    #expect(SidebarModel.summary(of: caption) == "3 hidden panes · 1 blocked")

    let open = SidebarModel.rows(roster: roster, agents: agents)
    #expect(open.map(\.label) == ["w1:p1", "Hidden"] + ids)
    #expect(SidebarModel.summary(of: open[1]) == nil)
  }

  /// A hidden helper of a pane with a row is listed under it, as the core says, folded into one
  /// line that counts them and paints the most urgent state among them and theirs (MIP-7).
  /// Opened, each helper's row follows a level further in, numbered by nothing, and a helper's
  /// own helpers fold under it in turn. They leave the Hidden caption, which keeps only what
  /// somebody hid by hand and is gone when nobody did.
  @Test("a pane's hidden helpers fold under it into one line, and open a level further in")
  func helpersFoldUnderTheirMaker() {
    let key = { PaneKey(daemon: "local", pane: $0) }
    let helper = { (id: String, maker: String) in
      Roster.Pane(
        key: key(id), label: id, onScreen: false, maker: maker, under: maker)
    }
    let helpersTab = tab(
      "local", "w1:t2", place: 0,
      panes: [helper("w1:h1", "w1:p1"), helper("w1:h2", "w1:p1"), helper("w1:g1", "w1:h1")])
    let base = roster([
      tab("local", panes: [pane("local", "w1:p1"), pane("local", "w1:p2", place: 2)])
    ])
    let roster = Roster(
      tabs: base.tabs, machines: base.machines, numbering: base.numbering, hidden: [helpersTab])
    let agents: [PaneKey: PaneAgent] = [
      key("w1:h1"): "working", key("w1:h2"): "idle", key("w1:g1"): "blocked",
    ]

    let folded = SidebarModel.rows(roster: roster, agents: agents)
    #expect(folded.map(\.label) == ["w1:p1", "2 helpers · 1 blocked", "w1:p2"])
    let line = folded[1]
    #expect(line.kind == .helpers(count: 2, urgent: 1, open: false))
    #expect(line.isHelpers && line.pane == key("w1:p1") && line.depth == 1)
    #expect(line.state == "blocked", "a blocked helper of a helper was lost behind the fold")
    #expect(SidebarModel.accessories(of: line) == [.helpers(open: false)])
    #expect(!folded.contains { $0.isHiddenCaption }, "only helpers were hidden")

    let open = SidebarModel.rows(roster: roster, agents: agents, helpersOpen: [key("w1:p1")])
    #expect(
      open.map(\.label) == [
        "w1:p1", "2 helpers", "w1:h1", "1 helper · 1 blocked", "w1:h2", "w1:p2",
      ])
    #expect(open[1].state.isEmpty, "an open line's rows carry their own states")
    #expect(open[2].hidden && open[2].kind == .pane(tabPress: 0, press: 0) && open[2].depth == 1)
    #expect(open[3].depth == 2 && open[3].pane == key("w1:h1"))
    #expect(
      roster.helpers(of: key("w1:p1")) == ContextMenuModel.Helpers(canShow: true, canHide: false))
    #expect(roster.helpers(of: key("w1:p2")) == nil, "a pane with no helpers has nothing to offer")

    // A pane somebody hid by hand stays under the caption, which counts only it.
    let byHand = tab(
      "local", "w1:t3", place: 0, panes: [pane("local", "w1:p9", place: 0, press: 0)])
    let both = Roster(
      tabs: base.tabs, machines: base.machines, numbering: base.numbering,
      hidden: [helpersTab, byHand])
    let rows = SidebarModel.rows(roster: both, agents: agents, hiddenCollapsed: true)
    #expect(rows.last?.kind == .hidden(panes: 1, urgent: 0, collapsed: true))
  }

  /// Hide Helpers takes back only a tab holding nothing but the pane's helpers, so a helper moved
  /// beside somebody's own panes does not make the item live.
  @Test("Hide Helpers is offered while a tab on screen holds nothing but the pane's helpers")
  func hideHelpersIsOfferedForATabOfHelpers() {
    let key = { PaneKey(daemon: "local", pane: $0) }
    let helper = { (id: String) in
      Roster.Pane(key: key(id), label: id, onScreen: false, maker: "w1:p1")
    }
    let mixed = roster([tab("local", panes: [pane("local", "w1:p1"), helper("w1:h1")])])
    #expect(
      mixed.helpers(of: key("w1:p1")) == ContextMenuModel.Helpers(canShow: false, canHide: false))
    let shown = roster([
      tab("local", panes: [pane("local", "w1:p1")]),
      tab("local", "w1:t2", place: 2, panes: [helper("w1:h1"), helper("w1:h2")]),
    ])
    #expect(
      shown.helpers(of: key("w1:p1")) == ContextMenuModel.Helpers(canShow: false, canHide: true))
  }

  @Test("a folded caption's dot is the most urgent state under it, and none for agents at rest")
  func theMostUrgentStateRanksAsTheCoreDoes() {
    #expect(SidebarModel.mostUrgent(["working", "done", "stuck", "blocked"]) == "blocked")
    #expect(SidebarModel.mostUrgent(["working", "done", "stuck"]) == "stuck")
    #expect(SidebarModel.mostUrgent(["working", "done"]) == "done")
    #expect(SidebarModel.mostUrgent(["idle", "working"]) == "working")
    #expect(SidebarModel.mostUrgent(["idle", "waiting", "unknown", "unreachable"]) == nil)
  }

  /// The callsign is the pane's handle and stays in view through a rename, drawn ahead of the name in
  /// a style of its own; an unnamed pane shows it once, not again inside its label.
  @MainActor
  @Test("every pane row leads with its callsign, named or not, and draws it once")
  func paneRowsLeadWithTheirCallsign() throws {
    let key = { PaneKey(daemon: "local", pane: $0) }
    let named = Roster.Pane(
      key: key("w1:p1"), place: 1, label: "🤖 builder", givenName: "🤖 builder", onScreen: true,
      callsign: "otter", besideCallsign: "🤖 builder")
    let unnamed = Roster.Pane(
      key: key("w1:p2"), place: 2, label: "maple · muster · claude", onScreen: true,
      callsign: "maple", besideCallsign: "muster · claude")
    let uncalled = Roster.Pane(key: key("w1:p3"), place: 3, label: "muster", onScreen: true)
    let rows = SidebarModel.rows(
      roster: roster([tab("local", panes: [named, unnamed, uncalled])]), agents: [:])

    let lines = rows.map { SidebarRowView.firstLine(for: $0, color: .labelColor).string }
    #expect(lines == ["otter  🤖 builder", "maple  muster · claude", "muster"])
    let first = SidebarRowView.firstLine(for: rows[0], color: .labelColor)
    let callsignFont = try #require(first.attribute(.font, at: 0, effectiveRange: nil) as? NSFont)
    let nameFont = try #require(
      first.attribute(.font, at: first.length - 1, effectiveRange: nil) as? NSFont)
    #expect(callsignFont.isFixedPitch && !nameFont.isFixedPitch, "the callsign is not set apart")

    #expect(SidebarModel.details(of: rows[0]).contains("callsign otter"))
    #expect(!SidebarModel.details(of: rows[1]).contains("callsign"))
  }

  @Test("a quiet pane's row is marked, wherever it is listed")
  func quietPanesAreMarked() {
    let quiet = Roster.Pane(
      key: PaneKey(daemon: "local", pane: "w1:p1"), place: 1, press: 1, label: "w1:p1",
      onScreen: true, quiet: true)
    let rows = SidebarModel.rows(roster: roster([tab("local", panes: [quiet])]), agents: [:])

    #expect(rows.first?.quiet == true)
    #expect(SidebarModel.accessories(of: rows[0]) == [.quiet])
  }

  /// While a window shows a group's conversation, the list says which panes are in the group:
  /// on screen or hidden, and on a machine that cannot be reached, since being a member is not
  /// something the agent is doing.
  @Test("the panes of a shown conversation's members are marked, and no others")
  func conversationMembersAreMarked() {
    let hiddenTab = tab(
      "local", "w1:t9", place: 0, panes: [pane("local", "w1:p9", place: 0, press: 0)])
    let base = roster([
      tab("local", panes: [pane("local", "w1:p1"), pane("local", "w1:p2", place: 2)])
    ])
    let roster = Roster(
      tabs: base.tabs, machines: base.machines, numbering: base.numbering, hidden: [hiddenTab])
    let members: Set<PaneKey> = [
      PaneKey(daemon: "local", pane: "w1:p1"), PaneKey(daemon: "local", pane: "w1:p9"),
    ]
    let agents: [PaneKey: PaneAgent] = [PaneKey(daemon: "local", pane: "w1:p9"): "unreachable"]
    let rows = SidebarModel.rows(roster: roster, agents: agents, members: members)

    let marked = rows.filter(\.member).map(\.label)
    #expect(marked == ["w1:p1", "w1:p9"])
    let byLabel = Dictionary(rows.map { ($0.label, $0) }, uniquingKeysWith: { first, _ in first })
    #expect(byLabel["w1:p1"].map(SidebarModel.accessories(of:)) == [.member])
    #expect(byLabel["w1:p9"].map(SidebarModel.accessories(of:)) == [.member])
    #expect(byLabel["w1:p2"].map(SidebarModel.accessories(of:)) == [])
    #expect(SidebarModel.details(of: byLabel["w1:p1"]!).contains("in the conversation shown"))
    #expect(
      SidebarModel.rows(roster: roster, agents: agents).allSatisfy { !$0.member },
      "a window showing its panes marks nothing")
  }

  @Test("a group says which machine keeps it while more than one is attached")
  func aGroupCarriesItsMachinesMark() {
    let rows = SidebarModel.rows(
      roster: roster([
        tab("local", panes: [pane("local", "w1:p1"), pane("devenv", "w1:p2", place: 2)])
      ]),
      agents: [:],
      groups: [SidebarModel.MessageGroup(daemon: "devenv", group: "g", unread: 1, toYou: 0)])
    let group = try? #require(rows.first { $0.isGroup })
    #expect(group?.machine == SidebarModel.MachineMark(daemon: "devenv", color: "#8d9440"))
  }

  @Test("a message arriving redraws its group's row and nothing else")
  func aMessageRedrawsOneRow() {
    let roster = roster([tab("local", panes: [pane("local", "w1:p1")])])
    let before = SidebarModel.rows(
      roster: roster, agents: [:],
      groups: [SidebarModel.MessageGroup(daemon: "local", group: "g", unread: 0, toYou: 0)])
    let after = SidebarModel.rows(
      roster: roster, agents: [:],
      groups: [SidebarModel.MessageGroup(daemon: "local", group: "g", unread: 1, toYou: 1)])
    let changed = try? #require(SidebarModel.changes(from: before, to: after))
    #expect(changed?.redraw == IndexSet([2]))
  }

  @Test("every row in the list is somewhere to go")
  func everyRowSelects() {
    // Clicking a caption shows that tab, which is the mouse's half of what ⌘N does; clicking a
    // pane row moves the keyboard there; and clicking a machine asks for a pane on it, which is
    // the only way into one holding nothing. There is no longer a row that names nothing.
    let roster = roster(
      [
        tab("local", "w1:t1", place: 1, panes: [pane("local", "w1:p1")]),
        tab("local", "w1:t2", place: 2, panes: [pane("local", "w1:p2")]),
      ],
      machines: [
        Roster.Machine(id: "local", state: "connected", panes: 2),
        Roster.Machine(id: "devenv", state: "connected", panes: 0),
      ])
    let rows = SidebarModel.rows(roster: roster, agents: [:])

    #expect(rows.allSatisfy { $0.isDestination })
    // Only the machine holding nothing gets a row: the one holding panes says so through them.
    #expect(rows.filter { $0.isMachine }.map(\.label) == ["devenv"])
  }

  /// A connected machine holding panes says nothing a row would add - until a restart stopped
  /// something there that still needs somebody.
  @Test("a machine whose restart still needs somebody gets a row saying what it cost")
  func aRestartedMachineGetsARow() {
    let restarted = "restarted: 2 panes started again, 1 agent stopped"
    let roster = roster(
      [tab("local", "w1:t1", place: 1, panes: [pane("local", "w1:p1")])],
      machines: [Roster.Machine(id: "local", state: "connected", panes: 1, restarted: restarted)])
    let rows = SidebarModel.rows(roster: roster, agents: [:]).filter { $0.isMachine }

    #expect(rows.map(\.label) == ["local"])
    #expect(rows.first?.subtitle == restarted)

    let settled = self.roster(
      [tab("local", "w1:t1", place: 1, panes: [pane("local", "w1:p1")])],
      machines: [Roster.Machine(id: "local", state: "connected", panes: 1)])
    #expect(SidebarModel.rows(roster: settled, agents: [:]).allSatisfy { !$0.isMachine })
  }

  @Test("the list takes its width, and gives it up before squeezing the panes")
  func theTerminalsWin() {
    let roomy = SidebarModel.widths(in: 960)
    #expect(roomy.sidebar == SidebarModel.width)
    #expect(roomy.sidebar + roomy.regions == 960)

    // A window too narrow for both drops the list rather than leaving a two-column terminal.
    // The terminals are what the app is for.
    let cramped = SidebarModel.widths(in: SidebarModel.width)
    #expect(cramped.sidebar == 0)
    #expect(cramped.regions == SidebarModel.width)
  }

  @Test("a list dragged wide takes at most half the window, and goes where it always went")
  func aDraggedWidthStillLeavesThePanesTheirRoom() {
    // The width it was dragged to, in a window with room for it.
    #expect(SidebarModel.widths(in: 1400, wanted: 320) == (320, 1080))
    // The same list in a window too small to give it that: half, not more, so a wide list
    // narrows with the window rather than leaving the panes a sliver.
    #expect(SidebarModel.widths(in: 500, wanted: 320) == (250, 250))
    // And it goes away below the width it always went away at, however it was dragged, so a
    // narrow window behaves the same whether or not anybody touched the list.
    #expect(SidebarModel.widths(in: 399, wanted: 140).sidebar == 0)
    #expect(SidebarModel.widths(in: 1400, shown: false, wanted: 320) == (0, 1400))
  }

  @Test("the panes beside the list start on a whole point, so their text is not resampled")
  func thePanesStartOnAWholePoint() {
    // A drag reports wherever the pointer was, and that is saved as it came.
    #expect(SidebarModel.widths(in: 1400, wanted: 261.58984375) == (261, 1139))
    // Half of an odd window is half a point.
    #expect(SidebarModel.widths(in: 501, wanted: 320) == (250, 251))
  }

  @MainActor
  @Test("dragging the list's edge asks for a width, and the list takes the one it is given")
  func theEdgeAsksAndTheLayoutAnswers() throws {
    let layout = WindowLayout(frame: NSRect(x: 0, y: 0, width: 1000, height: 400))
    let list = NSView()
    layout.attach(sidebar: list, strip: NSView())
    var asked: [CGFloat] = []
    layout.onSidebarDragged = { asked.append($0) }

    // The drag reports where the pointer is, which is the width being asked for. Nothing moves
    // yet: the list keeps the width the core last gave it until an answer arrives.
    layout.layoutSubtreeIfNeeded()
    let edge = try #require(layout.subviews.last as? SidebarEdgeView)
    let drag = try #require(
      NSEvent.mouseEvent(
        with: .leftMouseDragged, location: NSPoint(x: 312, y: 100), modifierFlags: [],
        timestamp: 0, windowNumber: 0, context: nil, eventNumber: 0, clickCount: 1, pressure: 1))
    edge.mouseDragged(with: drag)
    #expect(asked == [312])
    #expect(list.frame.width == SidebarModel.width)

    // The answer, which is what moves it - and the grip moves with the edge.
    layout.sidebarWidth = 312
    layout.layoutSubtreeIfNeeded()
    #expect(list.frame.width == 312)
    #expect(edge.frame.midX == 312)
  }

  @Test("the pane with the keyboard is marked, and only that one")
  func theKeyboardIsFindableInTheList() {
    // The list spans daemons and a window shows two of a dozen panes, so reading one back
    // against the other is hard. Marking the same pane in both is what joins them.
    let local = PaneKey(daemon: "local", pane: "w1:p1")
    let devenv = PaneKey(daemon: "devenv", pane: "w1:p1")
    let roster = roster([
      tab("local", panes: [Roster.Pane(key: local, label: "rad", onScreen: true)]),
      tab("devenv", place: 2, panes: [Roster.Pane(key: devenv, label: "rad", onScreen: true)]),
    ])

    let rows = SidebarModel.rows(roster: roster, agents: [:], keyboard: devenv)
    let marked = rows.filter(\.hasKeyboard)

    #expect(marked.count == 1, "more than one row claims the keyboard")
    // Two daemons hand out the same pane ids, so a list keyed on the id alone would light up
    // the laptop's row for a devenv pane.
    #expect(marked.first?.pane == devenv)
  }

  @Test("with the keyboard nowhere, no row claims it")
  func noRegionMeansNoMark() {
    let roster = roster([
      tab(
        "local",
        panes: [
          Roster.Pane(
            key: PaneKey(daemon: "local", pane: "w1:p1"), label: "rad", onScreen: false)
        ])
    ])
    #expect(
      SidebarModel.rows(roster: roster, agents: [:], keyboard: nil).allSatisfy { !$0.hasKeyboard })
  }

  @Test("a list put away gives its width to the panes, at any window size")
  func puttingItAwayIsNotTheSameAsRunningOutOfRoom() {
    // Two ways to end up with no list, and only one of them is a decision. Being asked for
    // it is remembered across launches; being too narrow is this window right now, and
    // widening the window again brings the list back because nothing about that was chosen.
    let away = SidebarModel.widths(in: 960, shown: false)
    #expect(away.sidebar == 0)
    #expect(away.regions == 960)

    let back = SidebarModel.widths(in: 960, shown: true)
    #expect(back.sidebar == SidebarModel.width)
  }

  @Test("a row carries what its agent is doing, and most rows carry nothing")
  func theSecondLineTravelsToTheRow() {
    let roster = roster([
      tab(
        "local",
        panes: [
          pane("local", "w1:p1", label: "muster · claude", subtitle: "first working build"),
          pane("local", "w1:p2", label: "src"),
        ])
    ])

    let panes = SidebarModel.rows(roster: roster, agents: [:]).filter { $0.isPane }
    #expect(panes.map(\.subtitle) == ["first working build", ""])
  }

  @Test("a waiting agent's second line is what it waits on")
  func aWaitingAgentSaysOnWhat() {
    let key = PaneKey(daemon: "local", pane: "w1:p1")
    let roster = roster([
      tab(
        "local",
        panes: [pane("local", "w1:p1", label: "muster · claude", subtitle: "first working build")])
    ])
    var waiting = PaneAgent(state: "waiting")
    waiting.waiting = "the full gate"
    let row = SidebarModel.rows(roster: roster, agents: [key: waiting]).first { $0.isPane }
    #expect(row?.state == "waiting")
    #expect(row?.subtitle == "waiting on the full gate")
    // Only while it waits: at work again, its title is its second line once more.
    var working = waiting
    working.state = "working"
    let back = SidebarModel.rows(roster: roster, agents: [key: working]).first { $0.isPane }
    #expect(back?.subtitle == "first working build")
  }

  @Test("a row's marks say only what there is to say, the ones asking to be looked at first")
  func marksAreThereOnlyWhenTheyAddSomething() {
    let key = PaneKey(daemon: "local", pane: "w1:p1")
    let roster = roster([tab("local", panes: [pane("local", "w1:p1")])])
    let plain = SidebarModel.rows(roster: roster, agents: [key: "working"]).first { $0.isPane }
    #expect(plain.map(SidebarModel.accessories(of:)) == [])

    var agent = PaneAgent(state: "working")
    agent.unreadable = true
    agent.rang = true
    agent.subagents = 2
    agent.contextUsed = 64
    let row = SidebarModel.rows(roster: roster, agents: [key: agent]).first { $0.isPane }
    #expect(
      row.map(SidebarModel.accessories(of:))
        == [.unreadable, .bell, .subagents(2), .context(64)])
    // A tab caption has no agent to have marks.
    let caption = SidebarModel.rows(
      roster: self.roster([
        tab("local", panes: [pane("local", "w1:p1")]), tab("local", "w1:t2", panes: []),
      ]),
      agents: [key: agent]
    ).first { $0.isTab }
    #expect(caption.map(SidebarModel.accessories(of:)) == [])
  }

  @Test("a pane whose machine cannot be reached says so, and what it was, and nothing current")
  func anUnreachablePaneSaysWhatItWas() {
    let key = PaneKey(daemon: "devenv", pane: "w1:p1")
    let roster = roster([tab("devenv", panes: [pane("devenv", "w1:p1")])])
    var agent = PaneAgent(state: "unreachable")
    agent.lastState = "blocked"
    agent.subagents = 2
    agent.contextUsed = 64
    agent.rang = true
    let row = SidebarModel.rows(roster: roster, agents: [key: agent]).first { $0.isPane }
    #expect(row?.subtitle == "unreachable · was blocked")
    #expect(row.map(SidebarModel.accessories(of:)) == [])
    #expect(
      row.map(SidebarModel.details(of:))?.contains("was blocked when last heard from") == true)
    // Painted as nothing in particular, the way a state the window cannot vouch for is.
    #expect(!PaneAppearance.isHighlighted(state: "unreachable"))
  }

  @Test("the context ring warms as the context fills, rather than flipping at a line")
  func theContextRingWarmsGradually() {
    let blends = stride(from: Float(0), through: 100, by: 5).map(SidebarModel.contextBlend(used:))
    #expect(blends.first == 0)
    #expect(blends.last == 1)
    // Every step moves it, which is what a threshold did not: 60% and 75% used to look alike.
    #expect(zip(blends, blends.dropFirst()).allSatisfy { $0 < $1 })
    // Calm while there is plenty left, and most of the way there by the old warning line.
    #expect(SidebarModel.contextBlend(used: 49) < 0.25)
    #expect(SidebarModel.contextBlend(used: 80) > 0.5)
    #expect(SidebarModel.contextBlend(used: -3) == 0)
    #expect(SidebarModel.contextBlend(used: 140) == 1)
  }

  @MainActor
  @Test("the context ring runs between its two ends, in light and in dark")
  func theContextRingRunsBetweenItsEnds() throws {
    defer { PaneAppearance.adopt(chrome: .none) }
    for name in [NSAppearance.Name.aqua, .darkAqua] {
      let appearance = try #require(NSAppearance(named: name))
      appearance.performAsCurrentDrawingAppearance {
        PaneAppearance.adopt(chrome: .none)
        expectSame(SidebarModel.contextColor(used: 0), SidebarModel.contextEmptyColor)
        expectSame(SidebarModel.contextColor(used: 100), SidebarModel.contextFullColor)

        // One end repainted leaves the other as shipped, the way one agent state does.
        PaneAppearance.adopt(
          chrome: Core.Chrome(
            divider: nil, focusRing: nil, agents: Core.AgentColors(), contextFull: "#00ff00"))
        expectSame(SidebarModel.contextColor(used: 100), NSColor(hex: "#00ff00")!)
        expectSame(SidebarModel.contextColor(used: 0), SidebarModel.contextEmptyColor)
      }
    }
  }

  @Test("a full context ring is not the colour of any agent state")
  func aFullRingIsNoAgentState() {
    // The ring sits beside the dot. It used to turn orange, which is `blocked`, so a nearly full
    // context could be read as an agent waiting on you.
    for state in ["working", "blocked", "waiting", "done", "idle", "unknown"] {
      #expect(SidebarModel.contextFullColor != PaneAppearance.defaultBorderColor(state: state))
    }
  }

  @Test("progress is drawn when it says how far, or that it failed, and not while it only spins")
  func progressIsDrawnWhenItSaysSomething() {
    let key = PaneKey(daemon: "local", pane: "w1:p1")
    let roster = roster([tab("local", panes: [pane("local", "w1:p1")])])
    func drawn(_ progress: PaneAgent.Progress) -> PaneAgent.Progress? {
      var agent = PaneAgent(state: "working")
      agent.progress = progress
      let row = SidebarModel.rows(roster: roster, agents: [key: agent]).first { $0.isPane }
      return row.flatMap(SidebarModel.progress(of:))
    }
    #expect(drawn(.init(state: "running", percent: 40))?.percent == 40)
    #expect(drawn(.init(state: "error", percent: nil))?.state == "error")
    // Some agents say this for as long as they work, which the dot already says.
    #expect(drawn(.init(state: "indeterminate", percent: nil)) == nil)
  }

  @Test("hovering a row says everything its agent said about itself")
  func detailsSayEverything() {
    let key = PaneKey(daemon: "local", pane: "w1:p1")
    let roster = roster([tab("local", panes: [pane("local", "w1:p1", label: "muster · claude")])])
    var agent = PaneAgent(state: "blocked")
    agent.reported = true
    agent.contextUsed = 64.4
    agent.subagents = 1
    agent.model = "Opus"
    agent.costUSD = 1.5
    let row = SidebarModel.rows(roster: roster, agents: [key: agent]).first { $0.isPane }
    #expect(
      row.map(SidebarModel.details(of:))
        == [
          "muster · claude", "blocked, as the agent reports", "64% of its context used",
          "1 sub-agent running", "Opus", "$1.50 so far",
        ].joined(separator: "\n"))
  }

  @Test("hovering a row whose adapter is not reporting says where to read about it")
  func detailsNameASilentAdapter() {
    let key = PaneKey(daemon: "local", pane: "w1:p1")
    let roster = roster([tab("local", panes: [pane("local", "w1:p1", label: "muster · codex")])])
    var agent = PaneAgent(state: "idle")
    agent.adapter = "silent"
    let row = SidebarModel.rows(roster: roster, agents: [key: agent]).first { $0.isPane }
    #expect(
      row.map(SidebarModel.details(of:))
        == ["muster · codex", "idle", "its adapter is not reporting (muster docs harnesses)"]
        .joined(separator: "\n"))
    agent.adapter = "none"
    let quiet = SidebarModel.rows(roster: roster, agents: [key: agent]).first { $0.isPane }
    #expect(quiet.map(SidebarModel.details(of:)) == "muster · codex\nidle")
  }

  @Test("hovering a stuck row names the program that stopped reading and what it left")
  func detailsNameAProgramThatStoppedReading() {
    let key = PaneKey(daemon: "local", pane: "w1:p1")
    let roster = roster([tab("local", panes: [pane("local", "w1:p1", label: "muster · codex")])])
    var agent = PaneAgent(state: "stuck")
    agent.unreadInput = .init(bytes: 2817, sinceMS: 1_700_000_000_000, program: "codex", pid: 4242)
    let row = SidebarModel.rows(roster: roster, agents: [key: agent]).first { $0.isPane }
    let lines = row.map(SidebarModel.details(of:))?.split(separator: "\n").map(String.init) ?? []
    #expect(lines.prefix(2) == ["muster · codex", "stuck"])
    // The time is the reader's own clock and locale, so only what surrounds it is pinned.
    #expect(lines.count == 3)
    #expect(lines.last?.hasPrefix("codex (pid 4242) has not read its input since ") == true)
    #expect(lines.last?.hasSuffix(" - 2817 bytes are waiting") == true)
  }

  @Test("a pane row is two lines whatever its agent says, and a caption is one")
  func panesAreTwoLinesAndCaptionsOne() {
    // The reason this is asserted rather than left to look right: a list of fifteen agents is
    // read by scanning it, and a height that varied with what an agent happened to be writing
    // would move every row below it whenever one of them wrote a longer sentence. A pane's
    // second line is its directory, which it always has, so no pane's height depends on its
    // agent at all.
    let roster = roster([
      tab(
        "local",
        panes: [
          pane("local", "w1:p1", subtitle: "first working build"),
          pane("local", "w1:p2", subtitle: "a very much longer sentence about the work"),
          pane("local", "w1:p3"),
        ]),
      tab("local", "w1:t2", place: 2, panes: [pane("local", "w1:p4", place: 2)]),
    ])

    let rows = SidebarModel.rows(roster: roster, agents: [:])
    #expect(rows.filter(\.isPane).map(SidebarModel.height(of:)) == Array(repeating: 40, count: 4))
    #expect(rows.filter(\.isTab).map(SidebarModel.height(of:)) == [20, 20])
    #expect(SidebarModel.twoLines == 40)
  }

  @Test("a rename starts from what somebody typed, not from what is drawn")
  func theGivenNameIsCarriedSeparately() {
    // `muster · claude` is composed here from a directory and a harness, and offering it as
    // the starting text of a rename would ask somebody to delete a name they never wrote. An
    // unnamed pane starts empty; a named one starts from its name and not from its caption,
    // which for a tab may carry the workspace in front of it.
    let roster = roster([
      Roster.Tab(
        id: "w1:t1", daemons: ["local"], place: 1, label: "one · release",
        onScreen: true, givenName: "release",
        panes: [
          pane("local", "w1:p1", label: "🔥 payments spike", givenName: "🔥 payments spike"),
          pane("local", "w1:p2", label: "muster · claude"),
        ]),
      tab("local", "w1:t2", place: 2, panes: [pane("local", "w1:p3")]),
    ])

    let rows = SidebarModel.rows(roster: roster, agents: [:])
    #expect(rows.first { $0.isTab }?.givenName == "release")
    #expect(rows.first { $0.pane?.pane == "w1:p1" }?.givenName == "🔥 payments spike")
    #expect(rows.first { $0.pane?.pane == "w1:p2" }?.givenName == "")
  }

  /// One daemon, one tab, the panes given. The shape most of the redraw cases want, where
  /// what is under test is what moved between two rosters rather than how they nest.
  private func roster(_ panes: [Roster.Pane]) -> Roster {
    roster([tab("local", panes: panes)])
  }

  /// A machine heading that has been restarted, which is the one row left whose second line comes
  /// and goes: its note says what the restart cost.
  private func machines(restarted: String) -> [Roster.Machine] {
    [
      Roster.Machine(
        id: "devenv", state: "connected", panes: 0, color: "#8d9440", restarted: restarted)
    ]
  }

  @Test("a second line arriving asks for the row to be measured again, not just drawn again")
  func aSecondLineArrivingAsksForAMeasure() {
    // The bug this pins: redrawing a row builds its view again inside the frame it already
    // had, so a row that grew a second line drew two lines in a one-line frame until
    // something unrelated reloaded the whole list. Both answers matter - the row has to be
    // drawn again because its words changed, and measured again because its height did.
    let panes = [pane("local", "w1:p1")]
    let before = SidebarModel.rows(
      roster: roster([tab("local", panes: panes)], machines: machines(restarted: "")),
      agents: [:])
    let after = SidebarModel.rows(
      roster: roster(
        [tab("local", panes: panes)], machines: machines(restarted: "restarted 2m ago")),
      agents: [:])

    let changed = SidebarModel.changes(from: before, to: after)
    // One tab, so no caption: the pane is row 0 and the machine is row 1.
    #expect(changed?.redraw == IndexSet(integer: 1))
    #expect(changed?.remeasure == IndexSet(integer: 1))
  }

  @Test("a second line leaving asks for the same thing")
  func aSecondLineLeavingAsksForAMeasure() {
    // The way back matters as much as the way there: a machine that stops saying what its
    // restart cost leaves a row that would otherwise keep a two-line frame with one line in it.
    let panes = [pane("local", "w1:p1")]
    let before = SidebarModel.rows(
      roster: roster(
        [tab("local", panes: panes)], machines: machines(restarted: "restarted 2m ago")),
      agents: [:])
    let after = SidebarModel.rows(
      roster: roster([tab("local", panes: panes)], machines: machines(restarted: "")),
      agents: [:])

    let changed = SidebarModel.changes(from: before, to: after)
    #expect(changed?.redraw == IndexSet(integer: 1))
    #expect(changed?.remeasure == IndexSet(integer: 1))
  }

  @Test("an agent blinking is drawn again and never measured again")
  func aBlinkCostsNoMeasure() {
    // The property the per-row redraw was bought with, and the one a careless fix for the
    // height would spend: a state change is the most frequent thing that happens in a window
    // full of agents, and none of them can move a row. Measuring on every blink would put
    // the cost back in a different place.
    let panes = [pane("local", "w1:p1"), pane("local", "w1:p2", place: 2)]
    let key = PaneKey(daemon: "local", pane: "w1:p1")
    let before = SidebarModel.rows(roster: roster(panes), agents: [key: "idle"])
    let after = SidebarModel.rows(roster: roster(panes), agents: [key: "working"])

    let changed = SidebarModel.changes(from: before, to: after)
    #expect(changed?.redraw == IndexSet(integer: 0))
    #expect(changed?.remeasure.isEmpty == true)
  }

  @Test("a pane opening asks for the whole list, because the rows are not the same rows")
  func aPaneOpeningReloadsEverything() {
    let before = SidebarModel.rows(roster: roster([pane("local", "w1:p1")]), agents: [:])
    let after = SidebarModel.rows(
      roster: roster([pane("local", "w1:p1"), pane("local", "w1:p2", place: 2)]), agents: [:])

    #expect(SidebarModel.changes(from: before, to: after) == nil)
  }

  @Test("a row that grows a second line is drawn at the taller height")
  @MainActor func theListRedrawsTallEnough() {
    // On the view rather than the model, because the model already answered this correctly
    // while the list drew it wrong: a table keeps the height it was last told until
    // something asks again, and only the frames it settles on say whether it was asked.
    let sidebar = SidebarView(
      frame: NSRect(x: 0, y: 0, width: SidebarModel.width, height: 400))
    let panes = [pane("local", "w1:p1", label: "muster · claude")]
    sidebar.apply(
      roster: roster([tab("local", panes: panes)], machines: machines(restarted: "")),
      agents: [:])
    #expect(sidebar.drawnRows.map(\.height) == [SidebarModel.twoLines, SidebarModel.oneLine])

    sidebar.apply(
      roster: roster(
        [tab("local", panes: panes)], machines: machines(restarted: "restarted 2m ago")),
      agents: [:])
    #expect(sidebar.drawnRows.map(\.height) == [SidebarModel.twoLines, SidebarModel.twoLines])
  }

  @Test("rows are drawn against the width the list was given, not the one it was born with")
  @MainActor func theListDrawsAtItsRealWidth() {
    // Built at zero and framed afterwards, which is the order the window uses: the list is a
    // stored property and its size arrives from a layout pass later. A row is laid out
    // against its own bounds, so a table left at the width it was born with would truncate
    // every label earlier than the list's width says and look like a labelling bug rather
    // than a sizing one. Written down because the shortcuts panel had to be told to follow
    // its clip view explicitly and says so in a comment, which makes this look like a
    // question the list has answered by luck. It has not - but the day somebody frames the
    // table by hand the way that panel does, the luck is what they would be spending.
    let sidebar = SidebarView(frame: .zero)
    sidebar.frame = NSRect(x: 0, y: 0, width: SidebarModel.width, height: 400)
    sidebar.layoutSubtreeIfNeeded()
    sidebar.apply(roster: roster([pane("local", "w1:p1", label: "muster · claude")]), agents: [:])

    #expect(sidebar.drawnRows.allSatisfy { $0.width == SidebarModel.width })
  }

  @Test("every row of a window holding one tab takes that tab along when dragged")
  func aLoneTabIsDraggedByAnyRow() {
    let rows = SidebarModel.rows(
      roster: roster([
        tab("local", "w1:t1", panes: [pane("local", "w1:p1"), pane("local", "w1:p2", place: 2)])
      ]), agents: [:])

    #expect(rows.allSatisfy { $0.isPane })
    #expect(rows.map { SidebarModel.draggedTab(of: $0, in: rows) } == ["w1:t1", "w1:t1"])
  }

  @Test("under a caption, only the caption takes a tab along")
  func aCaptionIsWhatMovesATab() {
    let rows = SidebarModel.rows(
      roster: roster([
        tab("local", "w1:t1", panes: [pane("local", "w1:p1")]),
        tab("local", "w1:t2", place: 2, panes: [pane("local", "w1:p2", place: 2)]),
      ]), agents: [:])

    #expect(
      rows.map { SidebarModel.draggedTab(of: $0, in: rows) } == ["w1:t1", nil, "w1:t2", nil])
  }

  @Test("a row carrying a pane and its tab is a pane at home and the tab elsewhere")
  func whereADragCameFromDecidesWhatItIs() {
    let key = PaneKey(daemon: "local", pane: "w1:p1")

    #expect(SidebarModel.drop(tab: "w1:t1", pane: key, fromThisWindow: true) == .pane(key))
    #expect(SidebarModel.drop(tab: "w1:t1", pane: key, fromThisWindow: false) == .tab("w1:t1"))
    #expect(SidebarModel.drop(tab: "w1:t1", pane: nil, fromThisWindow: true) == nil)
    #expect(SidebarModel.drop(tab: nil, pane: key, fromThisWindow: false) == nil)
  }

  @Test("a focus change moves the marker in the rows already drawn, rather than rebuilding them")
  @MainActor func aFocusChangeReusesTheRows() {
    let sidebar = SidebarView(
      frame: NSRect(x: 0, y: 0, width: SidebarModel.width, height: 400))
    let panes = [
      pane("local", "w1:p1"), pane("local", "w1:p2", place: 2), pane("local", "w1:p3", place: 3),
    ]
    sidebar.apply(roster: roster(panes), agents: [:], keyboard: panes[0].key)
    _ = sidebar.drawnRows
    let before = panes.indices.map { sidebar.rowView(at: $0) }
    #expect(before.allSatisfy { $0 != nil })
    #expect(before.map { $0?.marksKeyboard } == [true, false, false])

    sidebar.apply(roster: roster(panes), agents: [:], keyboard: panes[1].key)

    let after = panes.indices.map { sidebar.rowView(at: $0) }
    #expect(zip(before, after).allSatisfy { $0 === $1 })
    #expect(after.map { $0?.marksKeyboard } == [false, true, false])
  }

  @Test("a row shown again takes away what the row before it had")
  @MainActor func aReusedRowForgetsItsLastRow() {
    let sidebar = SidebarView(
      frame: NSRect(x: 0, y: 0, width: SidebarModel.width, height: 400))
    let busy = pane("local", "w1:p1", directory: "/Users/a/src/muster")
    var agent = PaneAgent(state: "blocked")
    agent.sinceMS = Int64((Date().timeIntervalSince1970 - 120) * 1000)
    agent.rang = true
    agent.contextUsed = 40
    agent.progress = PaneAgent.Progress(state: "running", percent: 30)
    sidebar.apply(roster: roster([busy]), agents: [busy.key: agent])
    _ = sidebar.drawnRows
    let row = sidebar.rowView(at: 0)
    #expect(row?.shownStateLine == "Blocked 2m")
    #expect(row?.shownDirectory == "muster")
    #expect(row?.shownIcon == "blocked")
    #expect(row?.showsTint == true)
    #expect(row?.shownMarks == 2)
    #expect(row?.shownContext == 40)
    #expect(row?.showsProgress == true)

    sidebar.apply(roster: roster([pane("local", "w1:p1")]), agents: [busy.key: "idle"])

    #expect(sidebar.rowView(at: 0) === row)
    #expect(row?.shownStateLine == nil)
    #expect(row?.shownDirectory == nil)
    #expect(row?.shownIcon == "idle")
    #expect(row?.showsTint == false)
    #expect(row?.shownMarks == 0)
    #expect(row?.shownContext == nil)
    #expect(row?.showsProgress == false)
  }

  /// Lets a test move the list's clock without waiting for it.
  private final class Clock {
    var now: Date
    init(_ now: Date) { self.now = now }
  }

  @Test(
    "what a change costs the list in redrawn rows, at one pane and at fifteen", arguments: [1, 15])
  @MainActor func redrawCost(panes count: Int) {
    let clock = Clock(now)
    let sidebar = SidebarView(frame: NSRect(x: 0, y: 0, width: SidebarModel.width, height: 800))
    sidebar.clock = { clock.now }
    let panes = (1...count).map { pane("local", "w1:p\($0)", place: $0, directory: "/src/p\($0)") }
    func agents(lastIs state: String) -> [PaneKey: PaneAgent] {
      var agents = Dictionary(
        uniqueKeysWithValues: panes.map { ($0.key, agent("idle", since: 90, now: clock.now)) })
      agents[panes[0].key] = agent("blocked", since: 90, now: clock.now)
      if count > 1 { agents[panes[1].key] = agent("done", since: 30, now: clock.now) }
      if count > 2 { agents[panes[count - 1].key] = agent(state, since: 90, now: clock.now) }
      return agents
    }
    let timed = min(count, 2)

    sidebar.apply(roster: roster(panes), agents: agents(lastIs: "idle"))
    #expect(sidebar.redrawCount == count, "the first draw is every row")
    #expect(sidebar.isTicking, "a blocked row shows a time, so the minute is being counted")

    // The same roster and states again, as a republish that changed nothing sends them.
    let before = sidebar.redrawCount
    sidebar.apply(roster: roster(panes), agents: agents(lastIs: "idle"))
    #expect(sidebar.redrawCount == before, "an unchanged republish redrew rows")

    // One agent changes what it is doing.
    if count > 2 {
      sidebar.apply(roster: roster(panes), agents: agents(lastIs: "working"))
      #expect(sidebar.redrawCount == before + 1, "a state change redrew more than its own row")
    }

    // A minute passes: only the rows showing a time say something different, and a few
    // seconds more change nothing.
    let afterChange = sidebar.redrawCount
    clock.now = clock.now.addingTimeInterval(60)
    sidebar.tick()
    #expect(sidebar.redrawCount == afterChange + timed, "the minute redrew rows with no time")
    clock.now = clock.now.addingTimeInterval(10)
    sidebar.tick()
    #expect(sidebar.redrawCount == afterChange + timed, "ten seconds redrew a row")
  }

  @Test("the minute is counted only while a row shows a time")
  @MainActor func theTimerRunsOnlyWhenNeeded() {
    let sidebar = SidebarView(frame: NSRect(x: 0, y: 0, width: SidebarModel.width, height: 400))
    let one = pane("local", "w1:p1")

    sidebar.apply(roster: roster([one]), agents: [one.key: agent("idle", since: 90)])
    #expect(!sidebar.isTicking)

    sidebar.apply(roster: roster([one]), agents: [one.key: agent("blocked", since: 90)])
    #expect(sidebar.isTicking)

    sidebar.apply(roster: roster([one]), agents: [one.key: agent("working", since: 90)])
    #expect(!sidebar.isTicking)
  }

  @Test("a caption draws its press as a key, and the tab on screen sits on a panel")
  @MainActor func captionsAreKeysOnAPanel() {
    let sidebar = SidebarView(frame: NSRect(x: 0, y: 0, width: SidebarModel.width, height: 400))
    let roster = roster(
      [
        tab("local", "w1:t1", press: 1, panes: [pane("local", "w1:p1", tabPress: 1, press: 1)]),
        tab(
          "local", "w1:t2", place: 2, press: 2, onScreen: true,
          panes: [pane("local", "w1:p2", place: 2, tabPress: 2, press: 1)]),
      ], numbering: .tabs)
    sidebar.apply(roster: roster, agents: [:])
    _ = sidebar.drawnRows
    let views = (0..<4).map { sidebar.rowView(at: $0) }

    // Caption, pane, caption, pane: the first tab's caption and pane carry their chords as keys.
    #expect(views[0]?.shownKeys == [1])
    #expect(views[1]?.shownKeys == [1, 1])
    #expect(views[2]?.shownKeys == [2])
    #expect(views[3]?.shownKeys == [2, 1])
    #expect(views.map { $0?.shownPanel } == [nil, nil, .top, .bottom])
  }

  /// An agent as the core last said it, `seconds` before `now`.
  private func agent(
    _ state: String, since seconds: Double? = nil,
    now: Date = Date(timeIntervalSince1970: 1_000_000),
    waiting: String = "", lastState: String = ""
  ) -> PaneAgent {
    var agent = PaneAgent(state: state)
    agent.sinceMS = seconds.map { Int64((now.timeIntervalSince1970 - $0) * 1000) } ?? 0
    agent.waiting = waiting
    agent.lastState = lastState
    return agent
  }

  private let now = Date(timeIntervalSince1970: 1_000_000)

  @Test("a state asks for the person as loudly as the legend says")
  func tiersFollowTheLegend() {
    let tiers = [
      "blocked": SidebarModel.Tier.asks, "stuck": .asks, "done": .look, "working": .calm,
      "waiting": .calm, "idle": .calm, "unknown": .absent, "unreachable": .absent,
      "a state nobody has invented yet": .absent,
    ]
    for (state, tier) in tiers {
      #expect(SidebarModel.Tier(state: state) == tier, "\(state)")
    }
  }

  @Test("line two says the state and how long it has held, but only where that wants somebody")
  func stateLines() {
    func line(_ agent: PaneAgent) -> String { SidebarModel.stateLine(for: agent, now: now) }

    #expect(line(agent("blocked", since: 65)) == "Blocked 1m")
    #expect(line(agent("stuck", since: 4 * 60)) == "Stuck 4m")
    #expect(line(agent("done", since: 2 * 3600)) == "Done 2h")
    // A calm agent says nothing: the row shows only where it is.
    #expect(line(agent("working", since: 30)).isEmpty)
    #expect(line(agent("idle", since: 30)).isEmpty)
    // What a waiting agent waits on says more than any time.
    #expect(line(agent("waiting", since: 30, waiting: "its build")) == "Waiting on its build")
    #expect(line(agent("waiting")) == "Waiting")
    #expect(line(agent("unknown")) == "Unknown")
    // What follows is what the agent said before its machine went, and no time: the daemon's
    // moment is when it stopped answering.
    #expect(
      line(agent("unreachable", since: 600, lastState: "working")) == "Unreachable · was working")
    #expect(line(agent("unreachable")) == "Unreachable")
    // The core has not said when: the word without a time, not a time of 55 years.
    #expect(line(agent("blocked")) == "Blocked")
  }

  @Test("how long a state has held is rounded down and never negative")
  func heldFor() {
    func held(_ seconds: Double) -> String {
      SidebarModel.heldFor(sinceMS: Int64((now.timeIntervalSince1970 - seconds) * 1000), now: now)
    }
    #expect(held(0) == "<1m")
    #expect(held(59) == "<1m")
    #expect(held(60) == "1m")
    #expect(held(3599) == "59m")
    #expect(held(3600) == "1h")
    #expect(held(86399) == "23h")
    #expect(held(86400) == "1d")
    #expect(held(-30) == "<1m", "a remote clock running ahead is not a negative time")
    #expect(SidebarModel.heldFor(sinceMS: 0, now: now).isEmpty)
  }

  @Test("a directory is shortened to its last part for the row and to ~ for the hover")
  func directoryShortening() {
    let home = "/Users/a"
    func name(_ path: String) -> String { SidebarModel.directoryName(path, home: home) }
    func full(_ path: String) -> String { SidebarModel.abbreviated(path, home: home) }

    #expect(name("/Users/a/src/muster") == "muster")
    #expect(name("/Users/a/src/muster/") == "muster", "a trailing slash is not a directory name")
    #expect(name("/Users/a") == "~")
    #expect(name("/Users/a/") == "~")
    #expect(name("/") == "/")
    #expect(name("").isEmpty)
    #expect(name("/Users/ab") == "ab", "a longer name that starts like home is not home")

    #expect(full("/Users/a/src/muster") == "~/src/muster")
    #expect(full("/Users/a") == "~")
    #expect(full("/Users/ab/src") == "/Users/ab/src")
    #expect(full("/srv/work") == "/srv/work")
  }

  @Test("a pane row carries its directory, tier and state line, hidden panes included")
  func rowsCarryTheSecondLine() {
    let shown = pane("local", "w1:p1", directory: "/Users/a/src/muster")
    let roster = Roster(
      tabs: [tab("local", panes: [shown])],
      machines: [
        Roster.Machine(id: "local", state: "connected", panes: 2, color: color(of: "local"))
      ],
      numbering: .panes,
      hidden: [
        Roster.Tab(
          id: "w1:t9", daemons: ["local"], place: 0, label: "", onScreen: false,
          panes: [pane("local", "w1:p9", place: 0, directory: "/srv/quiet/")])
      ])
    let agents = [
      shown.key: agent("blocked", since: 120, now: now),
      PaneKey(daemon: "local", pane: "w1:p9"): agent("done", since: 30, now: now),
    ]

    let rows = SidebarModel.rows(roster: roster, agents: agents, now: now)
    let first = rows.first { $0.pane == shown.key }
    let second = rows.first { $0.pane?.pane == "w1:p9" }

    #expect(first?.directory == "/Users/a/src/muster")
    #expect(first?.directoryName == "muster")
    #expect(first?.tier == .asks)
    #expect(first?.stateLine == "Blocked 2m")
    #expect(first?.isTinted == true)
    #expect(second?.directoryName == "quiet")
    #expect(second?.tier == .look)
    #expect(second?.stateLine == "Done <1m")
    #expect(second?.isTinted == false, "only a state that asks is tinted")
    #expect(rows.filter(\.isTab).allSatisfy { $0.tier == nil }, "a caption has no tier")
  }

  @Test("only blocked and stuck rows are tinted")
  func tintPerState() {
    for state in ["working", "waiting", "idle", "done", "unknown", "unreachable"] {
      let rows = SidebarModel.rows(
        roster: roster([pane("local", "w1:p1")]),
        agents: [PaneKey(daemon: "local", pane: "w1:p1"): agent(state)])
      #expect(rows[0].isTinted == false, "\(state)")
    }
    for state in ["blocked", "stuck"] {
      let rows = SidebarModel.rows(
        roster: roster([pane("local", "w1:p1")]),
        agents: [PaneKey(daemon: "local", pane: "w1:p1"): agent(state)])
      #expect(rows[0].isTinted, "\(state)")
    }
  }

  @Test("the tab on screen, its panes and the helper lines under them are one raised panel")
  func theShowingTabIsOnAPanel() {
    let roster = roster([
      tab("local", "w1:t1", press: 1, panes: [pane("local", "w1:p1")]),
      tab(
        "local", "w1:t2", place: 2, press: 2, onScreen: true,
        panes: [pane("local", "w1:p2", place: 2), pane("local", "w1:p3", place: 3)]),
      tab("local", "w1:t3", place: 3, press: 3, onScreen: false, panes: []),
    ])

    let rows = SidebarModel.rows(roster: roster, agents: [:])

    #expect(rows.map(\.panel) == [nil, nil, .top, .middle, .bottom, nil])
  }

  @Test(
    "a showing caption with nothing under it is a panel of one, and a window with no captions has none"
  )
  func panelEdgeCases() {
    let lonely = roster([
      tab("local", "w1:t1", press: 1, onScreen: true, panes: []),
      tab("local", "w1:t2", place: 2, press: 2, panes: [pane("local", "w1:p2", place: 2)]),
    ])
    #expect(SidebarModel.rows(roster: lonely, agents: [:]).map(\.panel).first == .only)

    let single = roster([tab("local", onScreen: true, panes: [pane("local", "w1:p1")])])
    #expect(SidebarModel.rows(roster: single, agents: [:]).allSatisfy { $0.panel == nil })
  }

  @Test("a minute passing redraws the rows that show a time and no others")
  func aMinuteRedrawsOnlyTimedRows() {
    let panes = [
      pane("local", "w1:p1"), pane("local", "w1:p2", place: 2), pane("local", "w1:p3", place: 3),
    ]
    let agents = [
      panes[0].key: agent("blocked", since: 70, now: now),
      panes[1].key: agent("working", since: 70, now: now),
      panes[2].key: agent("idle", since: 70, now: now),
    ]
    let before = SidebarModel.rows(roster: roster(panes), agents: agents, now: now)
    let later = SidebarModel.rows(
      roster: roster(panes), agents: agents, now: now.addingTimeInterval(60))
    let same = SidebarModel.rows(
      roster: roster(panes), agents: agents, now: now.addingTimeInterval(10))

    #expect(SidebarModel.changes(from: before, to: later)?.redraw == IndexSet(integer: 0))
    #expect(SidebarModel.changes(from: before, to: same)?.redraw.isEmpty == true)
  }

  @Test("the list is put away below the same window width it always was, whatever its default")
  func theCutoffDidNotMoveWithTheDefault() {
    #expect(SidebarModel.width == 280)
    #expect(SidebarModel.widths(in: 399).sidebar == 0)
    #expect(SidebarModel.widths(in: 400).sidebar == 200)
    #expect(SidebarModel.widths(in: 1000).sidebar == 280)
  }

  @Test("a pane's directory crosses the seam whole, beside a label that does not carry it")
  func directoryCrossesTheSeam() {
    var pane = Muster_RosterPane()
    pane.daemonID = "local"
    pane.paneID = "w1:p1"
    pane.label = "payments spike"
    pane.givenName = "payments spike"
    pane.directory = "/Users/a/src/muster"
    var tab = Muster_RosterTab()
    tab.tabID = "w1:t1"
    tab.panes = [pane]
    var changed = Muster_RosterChanged()
    changed.tabs = [tab]

    let listed = Roster(changed).tabs[0].panes[0]

    #expect(listed.directory == "/Users/a/src/muster")
    #expect(listed.label == "payments spike")
  }
}

/// Whether two colours paint the same under the current drawing appearance. Compared as sRGB
/// components because a dynamic colour and the mix it resolved to are different objects, and a
/// hair of tolerance because the mix is arithmetic on fractions.
private func expectSame(
  _ actual: NSColor, _ expected: NSColor, sourceLocation: SourceLocation = #_sourceLocation
) {
  guard let actual = actual.usingColorSpace(.sRGB), let expected = expected.usingColorSpace(.sRGB)
  else {
    Issue.record("a colour with no sRGB form", sourceLocation: sourceLocation)
    return
  }
  let components = { (color: NSColor) in
    [color.redComponent, color.greenComponent, color.blueComponent, color.alphaComponent]
  }
  #expect(
    zip(components(actual), components(expected)).allSatisfy { abs($0 - $1) < 0.002 },
    "\(actual) is not \(expected)", sourceLocation: sourceLocation)
}
