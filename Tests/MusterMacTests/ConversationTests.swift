import AppKit
import Testing

@testable import MusterMac

// What the shell does with a group's conversation: what it tells the core as the person writes,
// replies and sends, which key presses it takes, and how it words a member's row. What a
// conversation holds and what an edit makes of a draft are the core's, and are tested there
// (corpus/conformance/conversation.json and message_box.json); these are the wiring. No text
// view takes the keyboard here, for the input-method reason `SurfaceGUITests` gives.

@MainActor
final class ConversationRecorder {
  var sent: [String] = []
  var edits: [DraftEdit] = []
  /// What the core answers an edit with.
  var answer: (DraftEdit) -> Muster_MessageBoxDraft? = { edit in
    var draft = Muster_MessageBoxDraft()
    draft.text = edit.text
    draft.cursor = UInt32(edit.cursor)
    draft.urgent = edit.urgent
    draft.to = edit.to.map { recipient($0) }
    return draft
  }

  var requests: ConversationRequests {
    ConversationRequests(
      leave: { [self] in sent.append("leave") },
      openFold: { [self] in sent.append("fold \($0.group) \($1)") },
      showWhole: { [self] in sent.append("whole \($0.group) \($1)") },
      saveDraft: { [self] _, edit in
        edits.append(edit)
        return answer(edit)
      },
      post: { [self] in sent.append("post \($0.group)") },
      loadEarlier: { [self] in sent.append("earlier \($0.group)") },
      pause: { [self] in sent.append("pause \($0.group) \($1)") },
      focus: { [self] in sent.append("focus \($0.pane)") })
  }
}

private func recipient(_ name: String) -> Muster_Recipient {
  var recipient = Muster_Recipient()
  recipient.name = name
  recipient.shown = name
  return recipient
}

@MainActor
private func openBox(
  text: String = "", to: [String] = [], completions: [String] = [],
  sent: Muster_PostOutcome.State = .none
) -> (MessageBox, ConversationRecorder) {
  let recorder = ConversationRecorder()
  let box = MessageBox(requests: recorder.requests)
  var changed = Muster_MessageBoxChanged()
  changed.daemonID = "local"
  changed.group = "review"
  changed.draft.text = text
  changed.draft.to = to.map { recipient($0) }
  changed.draft.completions = completions.map { recipient($0) }
  changed.sent.state = sent
  box.apply(changed)
  return (box, recorder)
}

@MainActor
private func menuItem(_ action: String) -> NSMenuItem {
  NSMenuItem(title: action, action: Selector((action)), keyEquivalent: "")
}

private func member(
  _ name: String, liveness: Muster_ConversationMember.Liveness = .alive, pane: String = "",
  unread: (first: UInt64, from: String, why: String)? = nil
) -> Muster_ConversationMember {
  var member = Muster_ConversationMember()
  member.name = name
  member.shown = name
  member.liveness = liveness
  if !pane.isEmpty {
    member.daemonID = "local"
    member.paneID = pane
  }
  if let unread {
    member.unreadFirst = unread.first
    member.unreadFrom = unread.from
    member.unreadWhy = unread.why
  }
  return member
}

@Suite("a group's conversation in the shell")
@MainActor
struct ConversationTests {
  @Test("the message box takes the core's draft, and tells the field to show it")
  func theBoxTakesTheCoresDraft() {
    let (box, _) = openBox(text: "hello", to: ["otter"])
    #expect(box.group == ConversationGroup(daemon: "local", group: "review"))
    #expect(box.draft.text == "hello")
    #expect(box.recipients == ["otter"])
    #expect(box.revision == 1, "a draft from the core is one the field has not shown")
  }

  @Test("an edit hands the core the whole box, and its answer is the field's own")
  func anEditSendsTheWholeBox() {
    let (box, recorder) = openBox(to: ["otter"])
    box.setUrgent(true)
    let revision = box.revision

    let answer = box.edited(text: "ship it", cursor: 7)

    #expect(
      recorder.edits.last
        == DraftEdit(text: "ship it", cursor: 7, to: ["otter"], urgent: true))
    #expect(answer?.text == "ship it")
    #expect(box.revision == revision, "the field applies an answer to its own edit itself")
  }

  @Test("Reply addresses the author and keeps what is written")
  func replyAddressesTheAuthor() {
    let (box, recorder) = openBox(text: "on it", to: ["otter"])
    var focused = false
    box.onFocus = { focused = true }
    let revision = box.revision

    box.reply(to: "heron")

    #expect(recorder.edits.last?.to == ["heron"])
    #expect(recorder.edits.last?.text == "on it")
    #expect(box.revision == revision + 1, "the field shows the core's answer")
    #expect(focused, "and the keyboard goes to the box")
  }

  @Test("taking a recipient away hands the core the rest")
  func takingARecipientAway() {
    let (box, recorder) = openBox(to: ["otter", "heron"])
    box.remove(recipient: "otter")
    #expect(recorder.edits.last?.to == ["heron"])
  }

  @Test("Backspace after a recipient asks the core to turn it back into text")
  func backspaceUnaddresses() {
    let (box, recorder) = openBox(text: "go", to: ["heron"])
    box.unaddress(text: "go")
    #expect(recorder.edits.last?.unaddress == true)
    #expect(recorder.edits.last?.to == ["heron"], "the core decides which goes back")
  }

  @Test("a completion is the core's to make, and the highlight walks the list")
  func completionsAreTheCores() {
    let (box, recorder) = openBox(text: "@o", completions: ["olive", "otter"])
    box.moveHighlight(by: 1)
    #expect(box.highlighted == 1)
    box.moveHighlight(by: 1)
    #expect(box.highlighted == 0, "the highlight wraps")
    box.moveHighlight(by: -1)

    box.completeHighlighted(text: "@o", cursor: 2)

    #expect(recorder.edits.last?.complete == "otter")
    #expect(recorder.edits.last?.text == "@o")
  }

  @Test("Send posts a draft with words in it, once, and nothing while a post is on its way")
  func sendPostsOnce() {
    let (empty, emptyRecorder) = openBox(text: "  ")
    empty.send()
    #expect(emptyRecorder.sent.isEmpty, "an empty box posted")

    let (pending, pendingRecorder) = openBox(text: "yes", sent: .pending)
    pending.send()
    #expect(pendingRecorder.sent.isEmpty, "a box whose post is on its way posted again")

    let (box, recorder) = openBox(text: "yes")
    box.send()
    #expect(recorder.sent == ["post review"])
  }

  @Test("the box leaving the conversation empties it")
  func leavingEmptiesTheBox() {
    let (box, _) = openBox(text: "kept on the core")
    box.apply(Muster_MessageBoxChanged())
    #expect(box.group == nil)
    #expect(box.draft.text.isEmpty)
  }

  @Test("the members column keeps only its members' agents")
  func membersKeepTheirOwnAgents() {
    let members = ConversationMembers()
    var changed = Muster_ConversationMembersChanged()
    changed.members = [member("otter", pane: "p1"), member("@human", liveness: .human)]
    let p1 = PaneKey(daemon: "local", pane: "p1")
    let p2 = PaneKey(daemon: "local", pane: "p2")
    members.apply(changed, agents: [p1: "working", p2: "idle"], roster: Roster(tabs: []))
    #expect(members.agents == [p1: "working"])

    members.apply(pane: p2, agent: "blocked")
    #expect(members.agents[p2] == nil, "an agent outside the group was kept")
    members.apply(pane: p1, agent: "blocked")
    #expect(members.agents[p1] == "blocked")
  }

  @Test("a member's row says what its agent is doing, and why it has not read")
  func aMembersRow() {
    let otter = member(
      "otter", pane: "p1", unread: (4, "heron", "it is at a dialog, waiting for somebody"))
    #expect(
      MemberLine.doing(otter, agent: "working", subtitle: "fixing the build") == "fixing the build")
    var waiting = PaneAgent(state: "waiting")
    waiting.waiting = "./dev"
    #expect(MemberLine.doing(otter, agent: waiting, subtitle: "") == "waiting on ./dev")
    #expect(
      MemberLine.unread(otter)
        == "has not read what waits from heron: it is at a dialog, waiting for somebody")
    #expect(MemberLine.unread(member("heron")) == nil)
    #expect(
      MemberLine.doing(member("gone", liveness: .gone), agent: nil, subtitle: nil)
        == "no longer running")
    #expect(MemberLine.doing(member("@human", liveness: .human), agent: nil, subtitle: nil).isEmpty)
    #expect(
      MemberLine.state(member("far", liveness: .unreachable), agent: "working") == "unreachable")
  }

  @Test("Escape leaves the conversation, except while an input method composes")
  func escapeLeaves() {
    let leave = Core.Binding(action: "leave_conversation", key: "Escape", modifiers: [])
    #expect(
      ConversationKeys.meaning(key: "\u{1B}", modifiers: [], leave: leave, composing: false)
        == .leave)
    #expect(
      ConversationKeys.meaning(key: "\u{1B}", modifiers: [], leave: leave, composing: true)
        == .endComposition)
    #expect(
      ConversationKeys.meaning(key: "a", modifiers: [], leave: leave, composing: false) == .pass)
    #expect(
      ConversationKeys.meaning(key: "\u{1B}", modifiers: .shift, leave: leave, composing: false)
        == .pass)
    let rebound = Core.Binding(action: "leave_conversation", key: "KeyW", modifiers: ["control"])
    #expect(
      ConversationKeys.meaning(key: "\u{1B}", modifiers: [], leave: rebound, composing: false)
        == .pass, "Escape left after leaving was moved off it")
    #expect(
      ConversationKeys.meaning(key: "w", modifiers: .control, leave: rebound, composing: false)
        == .leave)
  }

  @Test("a cursor converts between the core's scalars and AppKit's UTF-16")
  func cursorsConvert() {
    let text = "a😀e\u{301}b"
    #expect(ScalarOffsets.utf16(beforeScalars: 2, in: text) == 3)
    #expect(ScalarOffsets.scalars(beforeUTF16: 3, in: text) == 2)
    #expect(ScalarOffsets.utf16(beforeScalars: 4, in: text) == 5)
    #expect(ScalarOffsets.scalars(beforeUTF16: text.utf16.count, in: text) == 5)
    #expect(ScalarOffsets.utf16(beforeScalars: 99, in: text) == text.utf16.count)
  }

  @Test("the message box undoes what was typed, and forgets it when the core rewrites the text")
  func undoForgetsWhatTheCoreRewrote() {
    let field = MessageBoxTextView(frame: .zero)
    field.allowsUndo = true
    field.insertText("hi", replacementRange: NSRange(location: 0, length: 0))
    field.breakUndoCoalescing()
    #expect(field.undoManager?.canUndo == true)
    #expect(field.validateUserInterfaceItem(menuItem("undo:")))

    var rewritten = Muster_MessageBoxDraft()
    rewritten.text = "hi there"
    rewritten.cursor = 8
    field.take(rewritten)

    #expect(field.undoManager?.canUndo == false, "Undo would replay against the old text")
    #expect(!field.validateUserInterfaceItem(menuItem("undo:")))
    #expect(field.string == "hi there")
  }

  @Test("the window's view says which conversation it shows")
  func theViewNamesTheConversation() {
    var changed = Muster_ViewChanged()
    #expect(WindowContents(changed).conversation == nil)
    changed.conversationDaemonID = "local"
    changed.conversationGroup = "review"
    #expect(
      WindowContents(changed).conversation == ConversationGroup(daemon: "local", group: "review"))
  }

  @Test("while a conversation covers the panes, nothing acts on the pane out of sight")
  func coveredPanesAreLeftAlone() {
    let close = #selector(MusterWindow.closePane(_:))
    let split = #selector(MusterWindow.splitRight(_:))
    let leave = #selector(MusterWindow.leaveConversation(_:))
    #expect(MusterWindow.isAvailable(close))
    #expect(!MusterWindow.isAvailable(close, covered: true))
    #expect(MusterWindow.isAvailable(split, covered: true), "making a pane leaves, so it stays")
    #expect(!MusterWindow.isAvailable(leave))
    #expect(MusterWindow.isAvailable(leave, covered: true))
  }

  @Test("two rows the core sent alike are still told apart")
  func rowsAreIdentifiedOnce() {
    var changes = Muster_ConversationRow()
    changes.changes.atMs = 5
    var message = Muster_ConversationRow()
    message.message.seq = 3
    let ids = identified([changes, message, changes]).map(\.id)
    #expect(ids == ["c:5", "m:3", "c:5#1"])
  }

  @Test("the menu leaves Escape to a pane", .ownsTheSeam)
  func theMenuLeavesEscapeToAPane() throws {
    let app = NSApplication.shared
    let (before, help) = (app.mainMenu, app.helpMenu)
    defer { (app.mainMenu, app.helpMenu) = (before, help) }
    AppMenu.install(
      target: NSObject(),
      bindings: [
        Core.Binding(
          action: "leave_conversation", key: "Escape", modifiers: [], inTextField: true)
      ])
    let escape = NSEvent.keyEvent(
      with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
      context: nil, characters: "\u{1B}", charactersIgnoringModifiers: "\u{1B}",
      isARepeat: false, keyCode: 0x35)
    #expect(app.mainMenu?.performKeyEquivalent(with: try #require(escape)) == false)
  }
}
