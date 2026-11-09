import Testing

@testable import MusterMac

@MainActor
@Suite("the banner stack folds all but the first, and keeps what is being answered")
struct BannerLookTests {
  private func banner(
    _ author: String, group: String = "g", seq: UInt64 = 1, held: Bool = false
  ) -> Muster_Banner {
    var banner = Muster_Banner()
    banner.daemonID = "local"
    banner.group = group
    banner.seq = seq
    banner.author = author
    banner.held = held
    return banner
  }

  @Test("the first banner is drawn in full and the rest are one line naming who they are from")
  func foldsPastTheFirst() {
    let banners = [
      banner("frost", group: "a"), banner("cactus", group: "b"), banner("lynx", group: "c"),
    ]
    let layout = BannerStackModel.layout(
      banners: banners, others: 0, keeps: { _ in false }, expanded: false)

    #expect(layout.full.map(\.author) == ["frost"])
    #expect(layout.folded.map(\.author) == ["cactus", "lynx"])
    #expect(layout.summary == "2 more, from cactus and lynx")
  }

  @Test("a banner being answered stays in full wherever it is in the stack")
  func keepsWhatIsBeingAnswered() {
    let banners = [banner("frost", group: "a"), banner("cactus", group: "b", held: true)]
    let layout = BannerStackModel.layout(
      banners: banners, others: 0, keeps: { $0.held }, expanded: false)

    #expect(
      layout.full.map(\.author) == ["frost", "cactus"], "the half-written reply is still there")
    #expect(layout.summary == nil)
  }

  @Test("the board keeps a banner whose reply holds words or the keyboard")
  func boardKeeps() {
    let board = BannerBoard()
    var typed = banner("cactus", group: "b")
    typed.replyable = true
    board.apply(bannersChanged([banner("frost", group: "a"), typed]))
    #expect(!board.keeps(typed))

    board.setReply("half a thought", for: typed)
    #expect(board.keeps(typed), "words written in it")
    board.setReply("", for: typed)
    board.focused = BannerBoard.key(daemon: "local", group: "b")
    #expect(board.keeps(typed), "the keyboard is in it")
  }

  private func bannersChanged(_ shown: [Muster_Banner]) -> Muster_BannersChanged {
    var changed = Muster_BannersChanged()
    changed.shown = shown
    return changed
  }

  @Test("Show draws every banner in full again")
  func expandedShowsAll() {
    let banners = [banner("a"), banner("b", group: "h"), banner("c", group: "i")]
    let layout = BannerStackModel.layout(
      banners: banners, others: 0, keeps: { _ in false }, expanded: true)
    #expect(layout.full.count == 3)
    #expect(layout.summary == nil)
  }

  @Test("banners the core holds back are counted, and named only as others")
  func othersAreCounted() {
    let banners = [banner("frost"), banner("cactus", group: "b")]
    let some = BannerStackModel.layout(
      banners: banners, others: 2, keeps: { _ in false }, expanded: false)
    #expect(some.summary == "3 more, from cactus and 2 others")

    let only = BannerStackModel.layout(
      banners: [banner("frost")], others: 4, keeps: { _ in false }, expanded: false)
    #expect(only.summary == "4 more")
    #expect(only.folded.isEmpty)
  }

  @Test("an author is found by callsign only when exactly one pane has it")
  func findsAnAuthor() {
    func pane(_ id: String, _ callsign: String) -> Roster.Pane {
      Roster.Pane(
        key: PaneKey(daemon: "local", pane: id), place: 1, tabPress: 0, press: 0, label: callsign,
        subtitle: "", givenName: "", onScreen: true, callsign: callsign, directory: "")
    }
    let tab = Roster.Tab(
      id: "w1:t1", daemons: ["local"], place: 1, press: 0, armed: false, label: "t",
      onScreen: true, folded: false,
      panes: [pane("w1:p1", "otter"), pane("w1:p2", "lynx"), pane("w1:p3", "lynx")])
    let roster = Roster(tabs: [tab], machines: [], numbering: .panes)
    let agents = [PaneKey(daemon: "local", pane: "w1:p1"): PaneAgent(state: "blocked")]

    #expect(
      BannerAuthor.find(callsign: "otter", roster: roster, agents: agents)
        == BannerAuthor(paneID: "w1:p1", state: "blocked"))
    #expect(BannerAuthor.find(callsign: "lynx", roster: roster, agents: agents) == nil)
    #expect(BannerAuthor.find(callsign: "linen", roster: roster, agents: agents) == nil)
    #expect(BannerAuthor.find(callsign: "", roster: roster, agents: agents) == nil)
  }

  @Test("the author text the core words for reading is taken back to who wrote it")
  func authorTextIsReadBack() {
    #expect(BannerAuthor.name(in: "otter via dashboard") == "otter")
    #expect(BannerAuthor.name(in: "otter") == "otter")
    #expect(BannerAuthor.isMuster("Muster"))
    #expect(BannerAuthor.isMuster("@muster"))
    #expect(!BannerAuthor.isMuster("musterfan"))
  }

  @Test("a lookup that moved, and only that, tells the stack to redraw")
  func authorsRedrawOnlyWhenTheyMove() {
    let authors = BannerAuthors()
    var state: String? = "working"
    let identify: (String) -> BannerAuthor? = { _ in BannerAuthor(paneID: "w1:p1", state: state) }
    let banners = [banner("otter")]

    authors.refresh(banners: banners, identify: identify)
    let first = authors.revision
    authors.refresh(banners: banners, identify: identify)
    #expect(authors.revision == first, "nothing moved")

    state = "blocked"
    authors.refresh(banners: banners, identify: identify)
    #expect(authors.revision == first + 1, "its agent changed state under a banner on screen")
  }
}
