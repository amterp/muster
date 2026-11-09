import Testing

@testable import MusterMac

// A banner for a conversation with one agent is named after that agent, not as `human+otter`
// (Fencil `one-to-one-on-agent-row`). The core words the author for reading, so these cases use
// what it sends: "otter", "otter via dashboard", "Muster".

@MainActor
@Suite("a one-to-one banner is named after its agent")
struct OneToOneBannerTests {
  @Test("a message from the agent needs nothing after its name")
  func theAgentsOwnMessage() {
    let caption = BannerCaption.make(
      author: "otter", conversation: "otter", group: "human+otter")
    #expect(caption.title == "otter")
    #expect(caption.suffix == nil)

    let viaProgram = BannerCaption.make(
      author: "otter via dashboard", conversation: "otter", group: "human+otter")
    #expect(viaProgram.title == "otter via dashboard")
    #expect(viaProgram.suffix == nil)
  }

  @Test("a message from somebody else in it says whose conversation it is")
  func someoneElsesMessage() {
    let caption = BannerCaption.make(
      author: "Muster", conversation: "otter", group: "human+otter")
    #expect(caption.title == "Muster")
    #expect(caption.suffix == "in otter")
  }

  @Test("a banner that could not fetch its message names the agent, not the group")
  func anUnloadedBanner() {
    let caption = BannerCaption.make(author: "", conversation: "otter", group: "human+otter")
    #expect(caption.title == "otter")
    #expect(caption.suffix == nil)
  }

  @Test("with no agent for it in this window, the group's name stays")
  func theGroupKeepsItsName() {
    let from = BannerCaption.make(author: "otter", conversation: nil, group: "human+otter")
    #expect(from.title == "otter")
    #expect(from.suffix == "in human+otter")
    let unloaded = BannerCaption.make(author: "", conversation: nil, group: "human+otter")
    #expect(unloaded.title == "human+otter")
    #expect(unloaded.suffix == nil)
  }

  @Test("the stack redraws when a banner's conversation gains or loses its agent's name")
  func theNameFollowsTheRoster() {
    let authors = BannerAuthors()
    var name: String? = "otter"
    var banner = Muster_Banner()
    banner.daemonID = "local"
    banner.group = "human+otter"
    banner.author = "otter"

    authors.refresh(banners: [banner], identify: { _ in nil }, conversation: { _, _ in name })
    let first = authors.revision
    authors.refresh(banners: [banner], identify: { _ in nil }, conversation: { _, _ in name })
    #expect(authors.revision == first, "nothing moved")

    name = nil
    authors.refresh(banners: [banner], identify: { _ in nil }, conversation: { _, _ in name })
    #expect(authors.revision == first + 1, "its pane left this window's roster")
  }

  @Test("the Needs you strip titles a one-to-one group's card with its agent's name")
  func theStripNamesTheAgent() throws {
    let key = PaneKey(daemon: "local", pane: "w1:p1")
    let pane = Roster.Pane(
      key: key, place: 1, tabPress: 0, press: 1, label: "otter · src", subtitle: "",
      onScreen: false, callsign: "otter")
    let roster = Roster(
      tabs: [
        Roster.Tab(
          id: "w1:t1", daemons: ["local"], place: 1, label: "t", onScreen: false, panes: [pane])
      ], machines: [])
    var raw = Muster_AskingEntry()
    raw.daemonID = "local"
    raw.group = "human+otter"
    raw.alert = "message"
    let entries = [AskingEntry(raw)]
    let named = SidebarModel.MessageGroup(
      daemon: "local", group: "human+otter", unread: 1, toYou: 1, partnerPane: key)
    let plain = SidebarModel.MessageGroup(
      daemon: "local", group: "human+otter", unread: 1, toYou: 1)

    let withPartner = NeedsYouModel.cards(
      entries: entries, roster: roster, agents: [:], groups: [named], now: .now)
    #expect(try #require(withPartner.first).title == "otter")
    let without = NeedsYouModel.cards(
      entries: entries, roster: roster, agents: [:], groups: [plain], now: .now)
    #expect(try #require(without.first).title == "human+otter")
  }
}
