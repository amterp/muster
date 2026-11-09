import AppKit
import Testing

@testable import MusterMac

// A conversation between the person and one agent with a pane sits on that agent's row, and the
// rest stay under Messages (Fencil `one-to-one-on-agent-row`). The core says the same partner to
// every window; each window decides from its own roster, and these cases pin that decision.

@MainActor
@Suite("a one-to-one conversation sits on its agent's row")
struct OneToOneTests {
  private func pane(_ id: String, callsign: String = "", daemon: String = "local") -> Roster.Pane {
    Roster.Pane(
      key: PaneKey(daemon: daemon, pane: id), place: 1, tabPress: 0, press: 1,
      label: callsign.isEmpty ? id : "\(callsign) · src", subtitle: "", onScreen: false,
      callsign: callsign)
  }

  private func tab(
    _ id: String = "w1:t1", folded: Bool = false, panes: [Roster.Pane]
  ) -> Roster.Tab {
    Roster.Tab(
      id: id, daemons: ["local"], place: 1, label: id, onScreen: false, folded: folded,
      panes: panes)
  }

  private func roster(_ panes: [Roster.Pane], hidden: [Roster.Pane] = []) -> Roster {
    Roster(
      tabs: [tab(panes: panes)], machines: [],
      hidden: hidden.isEmpty ? [] : [tab("w9:t9", panes: hidden)])
  }

  private func group(
    _ name: String, unread: Int = 0, toYou: Int = 0, daemon: String = "local",
    partner: PaneKey? = nil
  ) -> SidebarModel.MessageGroup {
    SidebarModel.MessageGroup(
      daemon: daemon, group: name, unread: unread, toYou: toYou, partnerPane: partner)
  }

  private let otter = PaneKey(daemon: "local", pane: "w1:p1")

  private func messagesRows(_ rows: [SidebarModel.Row]) -> [String] {
    rows.compactMap { row in
      if case .group = row.kind { return row.label }
      return nil
    }
  }

  @Test("a pair with a pane leaves Messages and puts its unread count on the pane's row")
  func aPairMovesToItsRow() throws {
    let pair = group("human+otter", unread: 2, toYou: 1, partner: otter)
    let roster = roster([pane("w1:p1", callsign: "otter")])

    let before = SidebarModel.rows(
      roster: roster, agents: [:], groups: [group("human+otter", unread: 2, toYou: 1)])
    #expect(messagesRows(before) == ["human+otter"])
    #expect(before.first { $0.isPane }?.conversation == nil)

    let after = SidebarModel.rows(roster: roster, agents: [:], groups: [pair])
    #expect(messagesRows(after).isEmpty)
    #expect(!after.contains { $0.kind == .messages }, "the caption goes with its last row")
    let row = try #require(after.first { $0.isPane })
    #expect(row.conversation?.unread == 2)
    #expect(row.conversation?.toYou == 1)
    #expect(
      SidebarModel.accessories(of: row).first == .conversation(unread: 2, toYou: 1),
      "the envelope is the first mark, nearest the name")
  }

  @Test("a pair with nothing unread still has its envelope, since the row is the way to it")
  func aQuietPairKeepsItsEnvelope() throws {
    let rows = SidebarModel.rows(
      roster: roster([pane("w1:p1", callsign: "otter")]), agents: [:],
      groups: [group("human+otter", partner: otter)])
    let row = try #require(rows.first { $0.isPane })
    #expect(SidebarModel.accessories(of: row).first == .conversation(unread: 0, toYou: 0))
    #expect(messagesRows(rows).isEmpty)
  }

  @Test("a pair whose partner's pane closed goes back under Messages")
  func aClosedPaneReturnsItsPair() {
    let pair = group("human+otter", unread: 1, partner: otter)
    let rows = SidebarModel.rows(
      roster: roster([pane("w1:p2", callsign: "lynx")]), agents: [:], groups: [pair])
    #expect(messagesRows(rows) == ["human+otter"])
    #expect(rows.allSatisfy { $0.conversation == nil })
  }

  @Test("each window decides from its own roster, so moving the agent's tab moves the envelope")
  func twoWindowsDecideSeparately() throws {
    let pair = group("human+otter", unread: 3, partner: otter)
    let holding = roster([pane("w1:p1", callsign: "otter")])
    let without = roster([pane("w2:p9", callsign: "lynx")])

    for (first, second) in [(holding, without), (without, holding)] {
      let a = SidebarModel.rows(roster: first, agents: [:], groups: [pair])
      let b = SidebarModel.rows(roster: second, agents: [:], groups: [pair])
      let badged = a.contains { $0.conversation != nil } ? a : b
      let listed = a.contains { $0.conversation != nil } ? b : a
      #expect(badged.first { $0.conversation != nil }?.conversation?.unread == 3)
      #expect(messagesRows(badged).isEmpty)
      #expect(messagesRows(listed) == ["human+otter"])
    }
  }

  @Test("a group with several members has no partner and stays under Messages")
  func aGroupOfSeveralStays() {
    let rows = SidebarModel.rows(
      roster: roster([pane("w1:p1", callsign: "otter")]), agents: [:],
      groups: [group("review", unread: 4)])
    #expect(messagesRows(rows) == ["review"])
    #expect(rows.allSatisfy { $0.conversation == nil })
  }

  @Test("the envelope opens the group on the group's machine, not the pane's")
  func theGroupIsKeptElsewhere() throws {
    let remote = PaneKey(daemon: "devenv", pane: "w1:p4")
    let rows = SidebarModel.rows(
      roster: roster([pane("w1:p4", callsign: "otter", daemon: "devenv")]), agents: [:],
      groups: [group("human+otter", unread: 1, daemon: "local", partner: remote)])
    let row = try #require(rows.first { $0.isPane })
    #expect(row.conversation?.daemon == "local")
    #expect(row.conversation?.group == "human+otter")
  }

  @Test("two groups naming one partner: the one with more unread takes the row")
  func theBusierPairTakesTheRow() throws {
    let old = group("human+otter", unread: 1, partner: otter)
    let current = group("human+otter-2", unread: 5, partner: otter)
    let rows = SidebarModel.rows(
      roster: roster([pane("w1:p1", callsign: "otter")]), agents: [:], groups: [old, current])
    let row = try #require(rows.first { $0.isPane })
    #expect(row.conversation?.group == "human+otter-2")
    #expect(messagesRows(rows) == ["human+otter"], "the other is still reachable")
  }

  @Test("a row nobody sees claims nothing: a folded tab, a collapsed Hidden caption")
  func anUndrawnRowKeepsTheGroupInMessages() throws {
    let pair = group("human+otter", unread: 1, partner: otter)
    let two = Roster(
      tabs: [
        tab("w1:t1", folded: true, panes: [pane("w1:p1", callsign: "otter")]),
        tab("w1:t2", panes: [pane("w1:p2", callsign: "lynx")]),
      ], machines: [])
    let folded = SidebarModel.rows(roster: two, agents: [:], groups: [pair])
    #expect(messagesRows(folded) == ["human+otter"])

    let open = Roster(
      tabs: [
        tab("w1:t1", panes: [pane("w1:p1", callsign: "otter")]),
        tab("w1:t2", panes: [pane("w1:p2", callsign: "lynx")]),
      ], machines: [])
    #expect(messagesRows(SidebarModel.rows(roster: open, agents: [:], groups: [pair])).isEmpty)

    let hidden = roster(
      [pane("w1:p2", callsign: "lynx")], hidden: [pane("w1:p1", callsign: "otter")])
    let collapsed = SidebarModel.rows(
      roster: hidden, agents: [:], groups: [pair], hiddenCollapsed: true)
    #expect(messagesRows(collapsed) == ["human+otter"])
    let expanded = SidebarModel.rows(
      roster: hidden, agents: [:], groups: [pair], hiddenCollapsed: false)
    #expect(messagesRows(expanded).isEmpty)
    #expect(expanded.first { $0.conversation != nil }?.pane == otter)
  }

  @Test("the name of a one-to-one conversation is its agent's callsign, when the window has it")
  func aConversationIsNamedAfterItsAgent() {
    let groups = [group("human+otter", partner: otter), group("review")]
    let here = roster([pane("w1:p1", callsign: "otter")])
    #expect(
      OneToOne.name(daemon: "local", group: "human+otter", groups: groups, roster: here)
        == "otter")
    #expect(OneToOne.name(daemon: "local", group: "review", groups: groups, roster: here) == nil)
    let elsewhere = roster([pane("w2:p9", callsign: "lynx")])
    #expect(
      OneToOne.name(daemon: "local", group: "human+otter", groups: groups, roster: elsewhere)
        == nil)
  }

  @Test("pressing the envelope opens the conversation and does not pick the pane")
  func theEnvelopeOpensTheGroup() throws {
    let sidebar = SidebarView(frame: NSRect(x: 0, y: 0, width: 280, height: 400))
    var opened: [String] = []
    var picked: [PaneKey] = []
    sidebar.onGroupPicked = { opened.append("\($0)/\($1)") }
    sidebar.onPanePicked = { picked.append($0) }
    sidebar.groups = [group("human+otter", unread: 2, partner: otter)]
    sidebar.apply(roster: roster([pane("w1:p1", callsign: "otter")]), agents: [:])

    let views = (0..<sidebar.drawnRows.count).compactMap { sidebar.rowView(at: $0) }
    let view = try #require(views.first { $0.conversationBadge != nil })
    let badge = try #require(view.conversationBadge)
    #expect(badge.unread == 2)
    badge.onPressed?()
    #expect(opened == ["local/human+otter"])
    #expect(picked.isEmpty)
    #expect(badge.toolTip?.contains("2 unread") == true)
  }

  @Test("a row's hover text says there is a conversation")
  func hoverTextNamesTheConversation() throws {
    let rows = SidebarModel.rows(
      roster: roster([pane("w1:p1", callsign: "otter")]), agents: [:],
      groups: [group("human+otter", unread: 2, toYou: 1, partner: otter)])
    let row = try #require(rows.first { $0.isPane })
    #expect(
      SidebarModel.details(of: row).contains("2 unread in your conversation with it, 1 to you"))
  }
}
