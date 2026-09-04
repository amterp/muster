import AppKit
import Testing

@testable import MusterMac

// The list is where the founding desideratum stops depending on what happens to be on
// screen. Its decisions are all joins and orderings, which is exactly the kind of thing that
// looks right in a screenshot and is wrong for one daemon out of two.

@Suite("the sidebar lists what exists")
struct SidebarTests {
  /// One pane, numbered as the core would have numbered it.
  ///
  /// The place is stated rather than counted here, for the same reason a tab's is: the core
  /// decides it, and a helper that numbered for itself would make these tests agree with a rule
  /// the shell does not follow. Defaults to one, since most cases here are about something else.
  ///
  /// The chord that reaches it defaults to its place, which is what the core sends under the
  /// scheme Muster ships. A case about `numbered_chords = "tab_then_pane"` states it instead,
  /// because that is the whole of what that scheme changes up here.
  private func pane(
    _ daemon: String, _ id: String, place: Int = 1, number: Int? = nil, label: String? = nil,
    subtitle: String = "", givenName: String = "", onScreen: Bool = false
  ) -> Roster.Pane {
    Roster.Pane(
      key: PaneKey(daemon: daemon, pane: id), place: place, number: number ?? place,
      label: label ?? id, subtitle: subtitle, givenName: givenName, onScreen: onScreen)
  }

  /// One tab, numbered as the core would have numbered it.
  ///
  /// The place is stated rather than counted here, because the core decides it - a helper that
  /// numbered for itself would make these tests agree with a rule the shell does not follow.
  ///
  /// No chord reaches it unless a case says one does: under the scheme Muster ships the
  /// numbers are on the panes, and a tab carrying one would be two numberings in one list.
  private func tab(
    _ daemon: String, _ id: String = "w1:t1", place: Int = 1, number: Int = 0,
    label: String? = nil, onScreen: Bool = false, panes: [Roster.Pane]
  ) -> Roster.Tab {
    Roster.Tab(
      id: id, daemons: [daemon], place: place, number: number, label: label ?? id,
      onScreen: onScreen, panes: panes)
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
      machines: machines ?? found.map { Roster.Machine(id: $0, state: "connected", panes: 1) },
      numbering: numbering)
  }

  @Test("panes from two machines sit in one flat list of tabs")
  func daemonsAreGrouped() {
    let roster = roster([
      tab("local", panes: [pane("local", "w1:p1")]),
      tab("devenv", place: 2, panes: [pane("devenv", "w1:p1")]),
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:])

    // No heading per machine: a tab may hold panes on two, so grouping by machine would be a
    // list that no longer describes the window beside it (MIP-2).
    #expect(rows.filter(\.isHeader).isEmpty)
    // The core's order, not re-sorted here. A list that sorted for itself would disagree
    // with the window it sits beside for any arrangement but the alphabetical one.
    #expect(rows.compactMap(\.pane?.daemon) == ["local", "devenv"])
    // And with two machines attached, every pane row says which it is on. With one it says
    // nothing, because the answer would be the same on every row.
    #expect(rows.filter { $0.isPane }.allSatisfy { $0.showsMachine })
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
    let before = SidebarModel.rows(roster: roster, states: [:])
    let after = SidebarModel.rows(
      roster: roster, states: [PaneKey(daemon: "local", pane: "w1:p2"): "working"])

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
      states: [
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
    let rows = SidebarModel.rows(roster: roster, states: [:])

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
  func hiddenPanesAreMarked() {
    let roster = roster([
      tab(
        "local",
        panes: [
          pane("local", "w1:p1", onScreen: true), pane("local", "w1:p9", onScreen: false),
        ])
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:]).filter { $0.isPane }

    #expect(rows.map(\.onScreen) == [true, false])
  }

  @Test("an empty roster draws nothing, not an empty heading")
  func nothingIsNothing() {
    // A window on the way up has an attached daemon and no panes yet. A heading over no rows
    // reads as a machine that lost its session.
    #expect(SidebarModel.rows(roster: roster([]), states: [:]).isEmpty)
    let bare = roster([])
    #expect(SidebarModel.rows(roster: bare, states: [:]).isEmpty)
  }

  @Test("a window with one tab draws no caption for it")
  func oneTabIsNotWorthALevel() {
    // The common case, and the reason this rule exists: with nothing to navigate between, a
    // row saying which tab you are in answers a question nobody has, and it costs a level of
    // indentation off every label in a 200pt column.
    let roster = roster([
      tab("local", onScreen: true, panes: [pane("local", "w1:p1")])
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:])

    #expect(rows.map(\.kind) == [.pane(number: 1)])
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
    let rows = SidebarModel.rows(roster: roster, states: [:])

    let kinds: [SidebarModel.Kind] = [
      .tab(number: 0), .pane(number: 1), .tab(number: 0), .pane(number: 2),
      .tab(number: 0), .pane(number: 3),
    ]
    #expect(rows.map(\.kind) == kinds)
    #expect(rows.filter { $0.isTab }.map(\.label) == ["one", "two", "three"])
  }

  @Test("under the prototype the numbers sit on the tabs, and on no pane")
  func theProtoypeNumbersTabsAtRest() {
    // What `numbered_chords = "tab_then_pane"` looks like before anything is pressed. The
    // point of drawing it at all is that a person can read what ⌘2 will do instead of
    // remembering what they last pressed - and the way that stays honest is that only one
    // kind of row carries a number at a time.
    let roster = roster(
      [
        tab(
          "local", "w1:t1", place: 1, number: 1, onScreen: true,
          panes: [pane("local", "w1:p1", place: 1, number: 0)]),
        tab(
          "local", "w1:t2", place: 2, number: 2,
          panes: [
            pane("local", "w1:p2", place: 2, number: 0),
            pane("local", "w1:p3", place: 3, number: 0),
          ]),
      ], numbering: .tabs)

    let rows = SidebarModel.rows(roster: roster, states: [:])

    let kinds: [SidebarModel.Kind] = [
      .tab(number: 1), .pane(number: 0), .tab(number: 2), .pane(number: 0), .pane(number: 0),
    ]
    #expect(rows.map(\.kind) == kinds)
    // Every tab and pane row leaves room for a digit, so that the press moving the numbers
    // inside a tab does not also drag every label in the list sixteen points sideways. This is
    // the list somebody reads to decide what to press, and it has to hold still while they do.
    #expect(rows.allSatisfy { $0.reservesNumber })
    // Nothing is half-typed yet, so the numbers are a reference rather than a live keystroke.
    #expect(rows.contains { $0.isSecondPress } == false)
  }

  @Test("once a chord has named a tab, that tab's panes are the numbered rows")
  func theProtoypeNumbersOneTabsPanes() {
    // The armed half, which is the same list a moment later: the numbers have moved inside
    // the tab that was named and left every other row without one. Nothing here decides that -
    // the core sends which row carries what, and this is the assertion that the list draws the
    // answer it was given rather than one of its own.
    let roster = roster(
      [
        tab(
          "local", "w1:t1", place: 1, number: 0,
          panes: [pane("local", "w1:p1", place: 1, number: 0)]),
        tab(
          "local", "w1:t2", place: 2, number: 0, onScreen: true,
          panes: [
            pane("local", "w1:p2", place: 2, number: 1),
            pane("local", "w1:p3", place: 3, number: 2),
          ]),
      ], numbering: .panesInTab)

    let rows = SidebarModel.rows(roster: roster, states: [:])

    let kinds: [SidebarModel.Kind] = [
      .tab(number: 0), .pane(number: 0), .tab(number: 0), .pane(number: 1), .pane(number: 2),
    ]
    #expect(rows.map(\.kind) == kinds)
    // The gutter is the same one it was a moment ago, which is the whole point of reserving
    // it: these two cases are one list one keystroke apart, and no label may have moved.
    #expect(rows.allSatisfy { $0.reservesNumber })
    // And a pane row's number now says what the very next press does, so it is drawn as the
    // keystroke it is rather than as something to look up. Captions carry none to emphasise.
    #expect(rows.filter { $0.isPane }.allSatisfy { $0.isSecondPress })
    #expect(rows.filter { $0.isTab }.contains { $0.isSecondPress } == false)
  }

  @Test("a numbered tab is drawn even in a window that would hide its caption")
  func oneTabStillShowsACaptionWhenItIsNumbered() {
    // A number nothing draws is a chord nobody can find, so the rule that a single tab needs
    // no caption gives way to a tab that carries one. The core no longer numbers the tab in a
    // window that holds only one - such a window numbers panes under either scheme, because
    // with one tab the two numberings produce the same numbers - so this pins the guard rather
    // than a state a running window reaches. Which rows carry numbers is the core's answer,
    // and the list has to draw whatever it is handed.
    let roster = roster([
      tab(
        "local", "w1:t1", place: 1, number: 1, onScreen: true,
        panes: [pane("local", "w1:p1", place: 1, number: 0)])
    ])

    let rows = SidebarModel.rows(roster: roster, states: [:])

    #expect(rows.map(\.kind) == [.tab(number: 1), .pane(number: 0)])
  }

  @Test("a row can only be dropped on a pane row belonging to the same daemon")
  func aDropStaysOnOneMachine() throws {
    // The case worth a test rather than a glance: two daemons hand out the same pane ids, so a
    // rule comparing ids alone calls a cross-machine drop legal and the core then resolves the
    // dragged pane against the wrong mirror and moves a different agent.
    let roster = roster([
      tab(
        "local", "w1:t1", place: 1, label: "here", onScreen: true,
        panes: [pane("local", "w1:p1", place: 1), pane("local", "w1:p2", place: 2)]),
      tab(
        "devenv", "w1:t1", place: 2, label: "there",
        panes: [pane("devenv", "w1:p1", place: 3)]),
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:])
    let dragged = PaneKey(daemon: "local", pane: "w1:p1")

    let sameDaemon = try #require(rows.first { $0.pane == PaneKey(daemon: "local", pane: "w1:p2") })
    let otherDaemon = try #require(
      rows.first { $0.pane == PaneKey(daemon: "devenv", pane: "w1:p1") })
    let caption = try #require(rows.first { $0.isTab })

    #expect(SidebarModel.canArrange(dragged, onto: sameDaemon))
    // Same pane id, different machine. This is the one a careless rule gets wrong.
    #expect(!SidebarModel.canArrange(dragged, onto: otherDaemon))
    #expect(!SidebarModel.canArrange(dragged, onto: caption))
  }

  @Test("dropping a row on itself is allowed and means nothing")
  func aDropOnItselfIsNotAnError() throws {
    // An accidental drag, which is a mistake with no cost. Refusing it would put a cursor
    // change and a log line in front of somebody who twitched, and the core answers it by
    // doing nothing.
    let roster = roster([
      tab("local", panes: [pane("local", "w1:p1", place: 1)])
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:])
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

    let rows = SidebarModel.rows(roster: roster, states: [:])

    #expect(
      rows.filter { $0.isPane }.map(\.kind)
        == [.pane(number: 1), .pane(number: 2), .pane(number: 3)])
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
    let rows = SidebarModel.rows(roster: roster, states: [:]).filter {
      if case .tab = $0.kind { return true }
      return false
    }

    #expect(rows.map(\.onScreen) == [true, false])
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
    let rows = SidebarModel.rows(roster: roster, states: [:])

    #expect(rows.allSatisfy { $0.isDestination })
    // Only the machine holding nothing gets a row: the one holding panes says so through them.
    #expect(rows.filter { $0.isMachine }.map(\.label) == ["devenv"])
  }

  @Test("the list takes a fixed width, and gives it up before squeezing the panes")
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

    let rows = SidebarModel.rows(roster: roster, states: [:], keyboard: devenv)
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
      SidebarModel.rows(roster: roster, states: [:], keyboard: nil).allSatisfy { !$0.hasKeyboard })
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

    let panes = SidebarModel.rows(roster: roster, states: [:]).filter { $0.isPane }
    #expect(panes.map(\.subtitle) == ["first working build", ""])
  }

  @Test("a row with a second line is taller, and only that row")
  func onlyASecondLineCostsHeight() {
    // The reason this is asserted rather than left to look right: a list of fifteen agents is
    // read by scanning it, and a height that varied with what an agent happened to be writing
    // would move every row below it whenever one of them wrote a longer sentence. Two heights
    // and no more means the only thing that can move a row is a title arriving or leaving.
    let roster = roster([
      tab(
        "local",
        panes: [
          pane("local", "w1:p1", subtitle: "first working build"),
          pane("local", "w1:p2", subtitle: "a very much longer sentence about the work"),
          pane("local", "w1:p3"),
        ])
    ])

    let panes = SidebarModel.rows(roster: roster, states: [:]).filter { $0.isPane }
    let heights = panes.map(SidebarModel.height(of:))
    #expect(heights == [SidebarModel.twoLines, SidebarModel.twoLines, SidebarModel.oneLine])
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

    let rows = SidebarModel.rows(roster: roster, states: [:])
    #expect(rows.first { $0.isTab }?.givenName == "release")
    #expect(rows.first { $0.pane?.pane == "w1:p1" }?.givenName == "🔥 payments spike")
    #expect(rows.first { $0.pane?.pane == "w1:p2" }?.givenName == "")
  }

  /// One daemon, one tab, the panes given. The shape most of the redraw cases want, where
  /// what is under test is what moved between two rosters rather than how they nest.
  private func roster(_ panes: [Roster.Pane]) -> Roster {
    roster([tab("local", panes: panes)])
  }

  @Test("a second line arriving asks for the row to be measured again, not just drawn again")
  func aSecondLineArrivingAsksForAMeasure() {
    // The bug this pins: redrawing a row builds its view again inside the frame it already
    // had, so a row that grew a second line drew two lines in a one-line frame until
    // something unrelated reloaded the whole list. Both answers matter - the row has to be
    // drawn again because its words changed, and measured again because its height did.
    let before = SidebarModel.rows(
      roster: roster([pane("local", "w1:p1"), pane("local", "w1:p2", place: 2)]), states: [:])
    let after = SidebarModel.rows(
      roster: roster([
        pane("local", "w1:p1", subtitle: "align-agent-state colours"),
        pane("local", "w1:p2", place: 2),
      ]), states: [:])

    let changed = SidebarModel.changes(from: before, to: after)
    // One tab, so no caption: the first pane is row 0.
    #expect(changed?.redraw == IndexSet(integer: 0))
    #expect(changed?.remeasure == IndexSet(integer: 0))
  }

  @Test("a second line leaving asks for the same thing")
  func aSecondLineLeavingAsksForAMeasure() {
    // The way back matters as much as the way there: an agent that stops titling itself
    // leaves a row that would otherwise keep a two-line frame with one line in it.
    let before = SidebarModel.rows(
      roster: roster([pane("local", "w1:p1", subtitle: "align-agent-state colours")]),
      states: [:])
    let after = SidebarModel.rows(roster: roster([pane("local", "w1:p1")]), states: [:])

    let changed = SidebarModel.changes(from: before, to: after)
    #expect(changed?.redraw == IndexSet(integer: 0))
    #expect(changed?.remeasure == IndexSet(integer: 0))
  }

  @Test("an agent blinking is drawn again and never measured again")
  func aBlinkCostsNoMeasure() {
    // The property the per-row redraw was bought with, and the one a careless fix for the
    // height would spend: a state change is the most frequent thing that happens in a window
    // full of agents, and none of them can move a row. Measuring on every blink would put
    // the cost back in a different place.
    let panes = [pane("local", "w1:p1"), pane("local", "w1:p2", place: 2)]
    let key = PaneKey(daemon: "local", pane: "w1:p1")
    let before = SidebarModel.rows(roster: roster(panes), states: [key: "idle"])
    let after = SidebarModel.rows(roster: roster(panes), states: [key: "working"])

    let changed = SidebarModel.changes(from: before, to: after)
    #expect(changed?.redraw == IndexSet(integer: 0))
    #expect(changed?.remeasure.isEmpty == true)
  }

  @Test("a pane opening asks for the whole list, because the rows are not the same rows")
  func aPaneOpeningReloadsEverything() {
    let before = SidebarModel.rows(roster: roster([pane("local", "w1:p1")]), states: [:])
    let after = SidebarModel.rows(
      roster: roster([pane("local", "w1:p1"), pane("local", "w1:p2", place: 2)]), states: [:])

    #expect(SidebarModel.changes(from: before, to: after) == nil)
  }

  @Test("a row that grows a second line is drawn at the taller height")
  @MainActor func theListRedrawsTallEnough() {
    // On the view rather than the model, because the model already answered this correctly
    // while the list drew it wrong: a table keeps the height it was last told until
    // something asks again, and only the frames it settles on say whether it was asked.
    let sidebar = SidebarView(
      frame: NSRect(x: 0, y: 0, width: SidebarModel.width, height: 400))
    sidebar.apply(roster: roster([pane("local", "w1:p1", label: "muster · claude")]), states: [:])
    #expect(sidebar.drawnRows.map(\.height) == [SidebarModel.oneLine])

    sidebar.apply(
      roster: roster([
        pane("local", "w1:p1", label: "muster · claude", subtitle: "align-agent-state colours")
      ]), states: [:])
    #expect(sidebar.drawnRows.map(\.height) == [SidebarModel.twoLines])
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
    sidebar.apply(roster: roster([pane("local", "w1:p1", label: "muster · claude")]), states: [:])

    #expect(sidebar.drawnRows.allSatisfy { $0.width == SidebarModel.width })
  }
}
