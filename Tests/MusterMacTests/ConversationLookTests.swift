import AppKit
import Testing

@testable import MusterMac

// What the restyled conversation takes from the palette and the roster. How it looks is judged by
// rendering it; these hold the parts that could be wrong without anybody seeing.

@MainActor
@Suite("the conversation's look")
struct ConversationLookTests {
  private func member(_ name: String, pane: String) -> Muster_ConversationMember {
    var member = Muster_ConversationMember()
    member.name = name
    member.shown = name
    member.liveness = .alive
    member.daemonID = "local"
    member.paneID = pane
    return member
  }

  @Test("a member's keys are the ones the roster gives its pane, as the agent list draws them")
  func chordsFromTheRoster() {
    let there = PaneKey(daemon: "local", pane: "w1:p2")
    let elsewhere = PaneKey(daemon: "local", pane: "w1:p9")
    let panes = [
      Roster.Pane(
        key: there, place: 2, tabPress: 2, press: 3, label: "otter", subtitle: "", givenName: "",
        onScreen: true, directory: ""),
      Roster.Pane(
        key: elsewhere, place: 3, tabPress: 1, press: 1, label: "lynx", subtitle: "",
        givenName: "", onScreen: true, directory: ""),
    ]
    let tab = Roster.Tab(
      id: "w1:t1", daemons: ["local"], place: 1, press: 0, armed: false, label: "t",
      onScreen: true, folded: false, panes: panes)
    let roster = Roster(tabs: [tab], machines: [], numbering: .panes)
    var changed = Muster_ConversationMembersChanged()
    changed.members = [member("otter", pane: "w1:p2"), member("linen", pane: "")]

    let members = ConversationMembers()
    members.apply(changed, agents: [:], roster: roster)

    #expect(members.chords[there] == ConversationMembers.Chord(first: 2, second: 3))
    #expect(members.chords[elsewhere] == nil, "only a member's keys are kept")
  }

  @Test("an avatar shows the first letter or digit of a name, past any leading @")
  func initials() {
    #expect(AvatarView.initial(of: "otter") == "O")
    #expect(AvatarView.initial(of: "@human") == "H")
    #expect(AvatarView.initial(of: "9lives") == "9")
    #expect(AvatarView.initial(of: "") == "?")
  }

  @Test("the message box types in the terminal's text color, not the Mac's")
  func fieldColor() throws {
    defer { PaneAppearance.adopt(chrome: .none) }
    PaneAppearance.adopt(chrome: .none, pane: .init(background: "#fdf6e3", foreground: "#073642"))
    PaneLookBoard.shared.refresh()
    let host = ConversationHost(requests: ConversationRecorder().requests)
    let container = NSView(frame: NSRect(x: 0, y: 0, width: 900, height: 560))
    host.show(ConversationGroup(daemon: "local", group: "g")) { view in
      view.frame = container.bounds
      container.addSubview(view)
    }
    let window = NSWindow(
      contentRect: container.bounds, styleMask: [.titled], backing: .buffered, defer: false)
    window.contentView = container
    container.layoutSubtreeIfNeeded()
    RunLoop.current.run(until: Date().addingTimeInterval(0.3))

    let hosting = try #require(host.hosting)
    let field = try #require(ConversationHost.field(in: hosting))
    #expect(field.textColor == PaneLookBoard.shared.look.surface.text.color)
  }
}
