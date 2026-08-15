import AppKit
import Testing

@testable import MusterMac

// The list is where the founding desideratum stops depending on what happens to be on
// screen. Its decisions are all joins and orderings, which is exactly the kind of thing that
// looks right in a screenshot and is wrong for one daemon out of two.

@Suite("the sidebar lists what exists")
struct SidebarTests {
  private func pane(_ daemon: String, _ id: String, label: String? = nil, onScreen: Bool = false)
    -> Roster.Pane
  {
    Roster.Pane(key: PaneKey(daemon: daemon, pane: id), label: label ?? id, onScreen: onScreen)
  }

  /// One tab, numbered as the core would have numbered it.
  ///
  /// The place is stated rather than counted here, because the core decides it - a helper that
  /// numbered for itself would make these tests agree with a rule the shell does not follow.
  private func tab(
    _ daemon: String, _ id: String = "w1:t1", place: Int = 1, label: String? = nil,
    onScreen: Bool = false, panes: [Roster.Pane]
  ) -> Roster.Tab {
    Roster.Tab(
      key: TabKey(daemon: daemon, tab: id), place: place, label: label ?? id, onScreen: onScreen,
      panes: panes)
  }

  @Test("each daemon's panes sit under a heading of their own")
  func daemonsAreGrouped() {
    let roster = Roster(daemons: [
      Roster.Daemon(id: "local", tabs: [tab("local", panes: [pane("local", "w1:p1")])]),
      Roster.Daemon(
        id: "devenv", tabs: [tab("devenv", place: 2, panes: [pane("devenv", "w1:p1")])]),
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:])

    #expect(rows.filter(\.isHeader).map(\.label) == ["local", "devenv"])
    // The core's order, not re-sorted here. A list that sorted for itself would disagree
    // with the window it sits beside for any arrangement but the alphabetical one.
    #expect(rows.compactMap(\.pane?.daemon) == ["local", "devenv"])
  }

  @Test("two daemons handing out one pane id are two rows")
  func paneIdsAreScopedToTheirDaemon() {
    // The whole reason a row is keyed by the pair. Collapsing these would put one machine's
    // agent state on the other's row, and clicking it would open the wrong pane.
    let roster = Roster(daemons: [
      Roster.Daemon(id: "local", tabs: [tab("local", panes: [pane("local", "w1:p1")])]),
      Roster.Daemon(
        id: "devenv", tabs: [tab("devenv", place: 2, panes: [pane("devenv", "w1:p1")])]),
    ])
    let rows = SidebarModel.rows(
      roster: roster,
      states: [
        PaneKey(daemon: "local", pane: "w1:p1"): "working",
        PaneKey(daemon: "devenv", pane: "w1:p1"): "blocked",
      ])

    let panes = rows.filter { $0.kind == .pane }
    #expect(panes.count == 2)
    #expect(panes.map(\.state) == ["working", "blocked"])
  }

  @Test("a pane the core has said nothing about is unknown, not idle")
  func silenceIsNotSuccess() {
    // The roster and the states are two messages and arrive in either order, so a list built
    // from the first alone has no state for anything. Reading that as idle would paint a
    // window full of running agents as a window with nothing to do.
    let roster = Roster(daemons: [
      Roster.Daemon(id: "local", tabs: [tab("local", panes: [pane("local", "w1:p1")])])
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:])

    #expect(rows.last?.state == "unknown")
    #expect(SidebarModel.dotColor(state: "unknown") != SidebarModel.dotColor(state: "idle"))
  }

  @Test("the dot agrees with the border the same state draws on a pane")
  func theListAgreesWithTheWindow() {
    // Two places show one state. A sidebar that picked its own palette would make a user
    // check both and trust neither.
    for state in ["working", "blocked", "done", "idle", "unknown"] {
      #expect(SidebarModel.dotColor(state: state) == PaneAppearance.borderColor(state: state))
    }
  }

  @Test("a row says whether anything is showing its pane")
  func hiddenPanesAreMarked() {
    let roster = Roster(daemons: [
      Roster.Daemon(
        id: "local",
        tabs: [
          tab(
            "local",
            panes: [
              pane("local", "w1:p1", onScreen: true), pane("local", "w1:p9", onScreen: false),
            ])
        ])
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:]).filter { $0.kind == .pane }

    #expect(rows.map(\.onScreen) == [true, false])
  }

  @Test("an empty roster draws nothing, not an empty heading")
  func nothingIsNothing() {
    // A window on the way up has an attached daemon and no panes yet. A heading over no rows
    // reads as a machine that lost its session.
    #expect(SidebarModel.rows(roster: Roster(daemons: []), states: [:]).isEmpty)
    let bare = Roster(daemons: [Roster.Daemon(id: "local", tabs: [])])
    #expect(SidebarModel.rows(roster: bare, states: [:]).isEmpty)
  }

  @Test("a window with one tab draws no caption for it")
  func oneTabIsNotWorthALevel() {
    // The common case, and the reason this rule exists: with nothing to navigate between, a
    // row saying which tab you are in answers a question nobody has, and it costs a level of
    // indentation off every label in a 200pt column.
    let roster = Roster(daemons: [
      Roster.Daemon(
        id: "local", tabs: [tab("local", onScreen: true, panes: [pane("local", "w1:p1")])])
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:])

    #expect(rows.map(\.kind) == [.daemon, .pane])
  }

  @Test("a second tab anywhere in the window gives every tab a caption")
  func tabsAppearTogetherOrNotAtAll() {
    // Including the tabs on a daemon that only holds one. The numbering counts across the
    // whole window, so showing it in patches would leave somebody counting rows that are not
    // there to work out what ⌘3 does.
    let roster = Roster(daemons: [
      Roster.Daemon(
        id: "local",
        tabs: [
          tab(
            "local", "w1:t1", place: 1, label: "one", onScreen: true,
            panes: [pane("local", "w1:p1")]),
          tab("local", "w1:t2", place: 2, label: "two", panes: [pane("local", "w1:p2")]),
        ]),
      Roster.Daemon(
        id: "devenv",
        tabs: [tab("devenv", "w1:t1", place: 3, label: "three", panes: [pane("devenv", "w1:p1")])]),
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:])

    #expect(
      rows.map(\.kind) == [
        .daemon, .tab(place: 1), .pane, .tab(place: 2), .pane, .daemon, .tab(place: 3), .pane,
      ])
    #expect(rows.filter { $0.kind == .tab(place: 3) }.map(\.label) == ["three"])
  }

  @Test("a tab caption says whether a region is showing it")
  func theTabOnScreenIsMarked() {
    // Which tab you are looking at is a different question from which pane you are typing
    // into, and in a two-region window they are two different tabs. The caption answers the
    // first; the keyboard highlight answers the second.
    let roster = Roster(daemons: [
      Roster.Daemon(
        id: "local",
        tabs: [
          tab("local", "w1:t1", place: 1, onScreen: true, panes: [pane("local", "w1:p1")]),
          tab("local", "w1:t2", place: 2, onScreen: false, panes: [pane("local", "w1:p2")]),
        ])
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:]).filter {
      if case .tab = $0.kind { return true }
      return false
    }

    #expect(rows.map(\.onScreen) == [true, false])
  }

  @Test("a tab caption is somewhere to go, and a daemon heading is not")
  func onlyTheReachableRowsSelect() {
    // Clicking a caption shows that tab, which is the mouse's half of what ⌘N does. A daemon
    // heading names no destination, and a highlight on one would suggest it had moved
    // something.
    let roster = Roster(daemons: [
      Roster.Daemon(
        id: "local",
        tabs: [
          tab("local", "w1:t1", place: 1, panes: [pane("local", "w1:p1")]),
          tab("local", "w1:t2", place: 2, panes: [pane("local", "w1:p2")]),
        ])
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:])

    #expect(rows.filter(\.isDestination).count == 4)
    #expect(rows.filter { !$0.isDestination }.map(\.kind) == [.daemon])
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
    let roster = Roster(daemons: [
      Roster.Daemon(
        id: "local",
        tabs: [tab("local", panes: [Roster.Pane(key: local, label: "rad", onScreen: true)])]),
      Roster.Daemon(
        id: "devenv",
        tabs: [
          tab("devenv", place: 2, panes: [Roster.Pane(key: devenv, label: "rad", onScreen: true)])
        ]),
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
    let roster = Roster(daemons: [
      Roster.Daemon(
        id: "local",
        tabs: [
          tab(
            "local",
            panes: [
              Roster.Pane(
                key: PaneKey(daemon: "local", pane: "w1:p1"), label: "rad", onScreen: false)
            ])
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
}
