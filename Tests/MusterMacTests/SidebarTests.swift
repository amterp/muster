import AppKit
import Testing

@testable import MusterMac

// The list is where the founding desideratum stops depending on what happens to be on
// screen. Its decisions are all joins and orderings, which is exactly the kind of thing that
// looks right in a screenshot and is wrong for one daemon out of two.

@Suite("the sidebar lists what exists")
struct SidebarTests {
  private func pane(
    _ daemon: String, _ id: String, label: String? = nil, tab: String = "w1:t1",
    onScreen: Bool = false
  ) -> Roster.Pane {
    Roster.Pane(
      key: PaneKey(daemon: daemon, pane: id), tab: tab, label: label ?? id, onScreen: onScreen)
  }

  @Test("each daemon's panes sit under a heading of their own")
  func daemonsAreGrouped() {
    let roster = Roster(panes: [
      pane("local", "w1:p1"), pane("local", "w1:p2"), pane("devenv", "w1:p1"),
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:])

    #expect(rows.map(\.isHeader) == [true, false, false, true, false])
    #expect(rows.compactMap(\.daemon) == ["local", "devenv"])
    // The core's order, not re-sorted here. A list that sorted for itself would disagree
    // with the window it sits beside for any arrangement but the alphabetical one.
    #expect(rows.compactMap(\.pane?.daemon) == ["local", "local", "devenv"])
  }

  @Test("two daemons handing out one pane id are two rows")
  func paneIdsAreScopedToTheirDaemon() {
    // The whole reason a row is keyed by the pair. Collapsing these would put one machine's
    // agent state on the other's row, and clicking it would open the wrong pane.
    let roster = Roster(panes: [pane("local", "w1:p1"), pane("devenv", "w1:p1")])
    let rows = SidebarModel.rows(
      roster: roster,
      states: [
        PaneKey(daemon: "local", pane: "w1:p1"): "working",
        PaneKey(daemon: "devenv", pane: "w1:p1"): "blocked",
      ])

    let panes = rows.filter { !$0.isHeader }
    #expect(panes.count == 2)
    #expect(panes.map(\.state) == ["working", "blocked"])
  }

  @Test("a pane the core has said nothing about is unknown, not idle")
  func silenceIsNotSuccess() {
    // The roster and the states are two messages and arrive in either order, so a list built
    // from the first alone has no state for anything. Reading that as idle would paint a
    // window full of running agents as a window with nothing to do.
    let rows = SidebarModel.rows(roster: Roster(panes: [pane("local", "w1:p1")]), states: [:])

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
    let roster = Roster(panes: [
      pane("local", "w1:p1", onScreen: true), pane("local", "w1:p9", onScreen: false),
    ])
    let rows = SidebarModel.rows(roster: roster, states: [:]).filter { !$0.isHeader }

    #expect(rows.map(\.onScreen) == [true, false])
  }

  @Test("an empty roster draws nothing, not an empty heading")
  func nothingIsNothing() {
    // A window on the way up has an attached daemon and no panes yet. A heading over no rows
    // reads as a machine that lost its session.
    #expect(SidebarModel.rows(roster: Roster(panes: []), states: [:]).isEmpty)
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
    let roster = Roster(panes: [
      Roster.Pane(key: local, tab: "w1:t1", label: "rad", onScreen: true),
      Roster.Pane(key: devenv, tab: "w1:t1", label: "rad", onScreen: true),
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
    let roster = Roster(panes: [
      Roster.Pane(
        key: PaneKey(daemon: "local", pane: "w1:p1"), tab: "w1:t1", label: "rad", onScreen: false)
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
