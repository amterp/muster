import AppKit
import Testing

@testable import MusterMac

// A conversation between the person and one agent with a pane sits on that agent's row (Fencil
// `one-to-one-on-agent-row`). No group has a row of its own, and every group is in the window's
// Conversations menu, whichever row carries it (Fencil `conversations-menus`). The core says the
// same partner to every window; each window decides from its own roster, and these cases pin that
// decision.

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

  /// The groups whose conversation some row carries as an envelope.
  private func onRows(_ rows: [SidebarModel.Row]) -> [String] {
    rows.compactMap(\.conversation?.group)
  }

  /// The groups the window's Conversations menu lists for `roster`.
  private func inWindowMenu(_ groups: [SidebarModel.MessageGroup], _ roster: Roster) -> [String] {
    ConversationsMenu.entries(groups: groups, roster: roster).map(\.group)
  }

  @Test("a pair with a pane puts its unread count on the pane's row, and no group has a row")
  func aPairSitsOnItsRow() throws {
    let pair = group("human+otter", unread: 2, toYou: 1, partner: otter)
    let roster = roster([pane("w1:p1", callsign: "otter")])

    let before = SidebarModel.rows(
      roster: roster, agents: [:], groups: [group("human+otter", unread: 2, toYou: 1)])
    #expect(before.allSatisfy { $0.isPane }, "a group with no partner took a row")
    #expect(before.first { $0.isPane }?.conversation == nil)

    let after = SidebarModel.rows(roster: roster, agents: [:], groups: [pair])
    #expect(after.allSatisfy { $0.isPane })
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
  }

  @Test("a pair whose partner's pane closed is on no row, and still in the window's menu")
  func aClosedPaneLeavesItsPairInTheMenu() {
    let pair = group("human+otter", unread: 1, partner: otter)
    let without = roster([pane("w1:p2", callsign: "lynx")])
    let rows = SidebarModel.rows(roster: without, agents: [:], groups: [pair])
    #expect(rows.allSatisfy { $0.isPane && $0.conversation == nil })
    #expect(inWindowMenu([pair], without) == ["human+otter"])
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
      #expect(onRows(listed).isEmpty)
    }
    #expect(inWindowMenu([pair], holding) == inWindowMenu([pair], without))
  }

  @Test("a group with several members is on no row, and in the window's menu under its name")
  func aGroupOfSeveralIsInTheMenu() {
    let here = roster([pane("w1:p1", callsign: "otter")])
    let groups = [group("review", unread: 4)]
    let rows = SidebarModel.rows(roster: here, agents: [:], groups: groups)
    #expect(rows.allSatisfy { $0.isPane && $0.conversation == nil })
    #expect(
      ConversationsMenu.entries(groups: groups, roster: here).map(\.title)
        == ["review (4 unread)"])
    #expect(
      ConversationsMenu.entries(groups: groups, roster: here, pane: otter).isEmpty,
      "the core names no members of a group of several, so no agent's menu can list it")
  }

  @Test("the window's menu names a one-to-one conversation after its agent, an agent's by group")
  func menusTitleAConversation() {
    let here = roster([pane("w1:p1", callsign: "otter"), pane("w1:p2", callsign: "lynx")])
    let groups = [
      group("human+lynx", partner: PaneKey(daemon: "local", pane: "w1:p2")),
      group("human+otter", unread: 1, partner: otter),
      group("human+otter-2", partner: otter),
      group("review", daemon: "devenv"),
      group("review"),
    ]
    #expect(
      ConversationsMenu.entries(groups: groups, roster: here).map(\.title) == [
        "lynx", "otter (human+otter) (1 unread)", "otter (human+otter-2)", "review on devenv",
        "review on local",
      ])
    // Listed in the core's order, each naming the machine that keeps it.
    #expect(
      ConversationsMenu.entries(groups: groups, roster: here).map(\.daemon) == [
        "local", "local", "local", "devenv", "local",
      ])
    let otters = ConversationsMenu.entries(groups: groups, roster: here, pane: otter)
    #expect(otters.map(\.title) == ["human+otter (1 unread)", "human+otter-2"])
    #expect(
      ConversationsMenu.entries(
        groups: groups, roster: here, pane: PaneKey(daemon: "local", pane: "w1:p9")
      ).isEmpty)
    #expect(ConversationsMenu.entries(groups: [], roster: here).isEmpty)
  }

  @Test("a group's submenu opens it, leaves it or deletes it, and an empty list makes no menu")
  func aGroupsSubmenuOffersItsThreeVerbs() throws {
    let here = roster([pane("w1:p1", callsign: "otter")])
    let entries = ConversationsMenu.entries(
      groups: [group("human+otter", partner: otter), group("review")], roster: here)
    var picked: [String] = []
    let item = try #require(
      ConversationsMenu.item(entries) { choice, entry in
        picked.append("\(choice) \(entry.daemon)/\(entry.group)")
      })
    #expect(item.title == "Conversations")
    let groups = try #require(item.submenu?.items)
    #expect(groups.map(\.title) == ["otter", "review"])
    let verbs = try #require(groups[1].submenu?.items)
    #expect(
      verbs.map { $0.isSeparatorItem ? "-" : $0.title }
        == ["Open Conversation", "Leave Group", "-", "Delete Group…"])
    for verb in verbs where !verb.isSeparatorItem {
      _ = verb.target?.perform(verb.action, with: verb)
    }
    #expect(picked == ["open local/review", "leave local/review", "delete local/review"])
    #expect(ConversationsMenu.item([]) { _, _ in } == nil)
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
    let reachable = ConversationsMenu.entries(
      groups: [old, current], roster: roster([pane("w1:p1", callsign: "otter")]), pane: otter)
    #expect(
      reachable.map(\.group) == ["human+otter", "human+otter-2"],
      "the other is still reachable, from the agent's own menu")
  }

  @Test("a row nobody sees claims nothing: a folded tab, a collapsed Hidden caption")
  func anUndrawnRowClaimsNothing() throws {
    let pair = group("human+otter", unread: 1, partner: otter)
    let two = Roster(
      tabs: [
        tab("w1:t1", folded: true, panes: [pane("w1:p1", callsign: "otter")]),
        tab("w1:t2", panes: [pane("w1:p2", callsign: "lynx")]),
      ], machines: [])
    let folded = SidebarModel.rows(roster: two, agents: [:], groups: [pair])
    #expect(onRows(folded).isEmpty)
    #expect(inWindowMenu([pair], two) == ["human+otter"], "the menu still reaches it")

    let open = Roster(
      tabs: [
        tab("w1:t1", panes: [pane("w1:p1", callsign: "otter")]),
        tab("w1:t2", panes: [pane("w1:p2", callsign: "lynx")]),
      ], machines: [])
    #expect(
      onRows(SidebarModel.rows(roster: open, agents: [:], groups: [pair])) == ["human+otter"])

    let hidden = roster(
      [pane("w1:p2", callsign: "lynx")], hidden: [pane("w1:p1", callsign: "otter")])
    let collapsed = SidebarModel.rows(
      roster: hidden, agents: [:], groups: [pair], hiddenCollapsed: true)
    #expect(onRows(collapsed).isEmpty)
    let expanded = SidebarModel.rows(
      roster: hidden, agents: [:], groups: [pair], hiddenCollapsed: false)
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

  @Test("the envelope carries the group's menu, and VoiceOver's press opens the conversation")
  func theEnvelopeIsMenuedAndPressable() throws {
    let sidebar = SidebarView(frame: NSRect(x: 0, y: 0, width: 280, height: 400))
    var opened: [String] = []
    var asked: [String] = []
    sidebar.onGroupPicked = { opened.append("\($0)/\($1)") }
    sidebar.onConversationMenu = {
      asked.append("\($0.daemon)/\($0.group)")
      return NSMenu(title: "group")
    }
    sidebar.groups = [group("human+otter", unread: 1, partner: otter)]
    sidebar.apply(roster: roster([pane("w1:p1", callsign: "otter")]), agents: [:])

    let views = (0..<sidebar.drawnRows.count).compactMap { sidebar.rowView(at: $0) }
    let badge = try #require(views.compactMap(\.conversationBadge).first)
    let click = try #require(
      NSEvent.mouseEvent(
        with: .rightMouseDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
        context: nil, eventNumber: 0, clickCount: 1, pressure: 1))
    #expect(badge.menu(for: click)?.title == "group", "Leave Group and Delete Group live here")
    #expect(asked == ["local/human+otter"])

    #expect(badge.accessibilityPerformPress())
    #expect(opened == ["local/human+otter"])
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
