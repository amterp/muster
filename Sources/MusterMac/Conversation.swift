import AppKit
import SwiftUI

// A group's conversation, drawn in place of a window's panes (mip/0009-group-conversation.md,
// sections 1, 3 and 7). Everything it shows is the core's: the header and the thread's rows
// (`ConversationChanged`), the members column (`ConversationMembersChanged`), and the message
// box's draft and where a post from it stands (`MessageBoxChanged`). This file draws their last
// word and says what the person did. Each event has a model of its own, so that typing in the
// box or an agent changing state never redraws the thread.
//
// Hosted in the AppKit window on the terms `FindBar.swift` sets out: the keyboard goes into the
// message box through AppKit rather than `@FocusState`.

/// A group as a window names the conversation it shows: the daemon keeping it here, and its name
/// there.
public struct ConversationGroup: Equatable, Hashable, Sendable {
  public let daemon: String
  public let group: String

  public init(daemon: String, group: String) {
    self.daemon = daemon
    self.group = group
  }
}

/// The message box as the person left it, for the core to take in.
struct DraftEdit: Equatable {
  var text: String
  /// Where the cursor is, in Unicode scalar values.
  var cursor: Int
  var to: [String]
  var urgent: Bool
  var unaddress = false
  /// The member to complete the @name at the cursor to, by participant name.
  var complete = ""
}

/// What a conversation asks of the core, as functions so a test can see them without one.
struct ConversationRequests {
  var leave: @MainActor () -> Void
  var openFold: @MainActor (ConversationGroup, UInt64) -> Void
  var showWhole: @MainActor (ConversationGroup, UInt64) -> Void
  var saveDraft: @MainActor (ConversationGroup, DraftEdit) -> Muster_MessageBoxDraft?
  /// Picks question #seq's answer by its place, or takes the pick back with nil.
  var pick: @MainActor (ConversationGroup, UInt64, UInt32?) -> Muster_MessageBoxDraft?
  var post:
    @MainActor (ConversationGroup, _ then: @escaping @MainActor @Sendable () -> Void) -> Void
  var loadEarlier: @MainActor (ConversationGroup) -> Void
  var pause: @MainActor (ConversationGroup, Bool) -> Void
  var focus: @MainActor (PaneKey) -> Void

  /// Through the core, each naming the window `window` says, which need not be the one in front.
  static func core(window: @escaping @MainActor () -> String) -> ConversationRequests {
    ConversationRequests(
      leave: { Core.speaking(for: window()) { Core.leaveConversation() } },
      openFold: { group, first in
        Core.speaking(for: window()) { Core.openFold(group, first: first) }
      },
      showWhole: { group, seq in
        Core.speaking(for: window()) { Core.showWholeMessage(group, seq: seq) }
      },
      saveDraft: { group, edit in Core.speaking(for: window()) { Core.saveDraft(group, edit) } },
      pick: { group, question, answer in
        Core.speaking(for: window()) {
          Core.pickConversationAnswer(group, question: question, answer: answer)
        }
      },
      post: { group, then in Core.postFromConversation(group, window: window(), then: then) },
      loadEarlier: { group in Core.speaking(for: window()) { Core.loadEarlier(group) } },
      pause: { group, paused in
        Core.speaking(for: window()) { Core.pauseGroup(group, paused: paused) }
      },
      focus: { pane in
        Core.speaking(for: window()) { Core.focus(daemonID: pane.daemon, paneID: pane.pane) }
      })
  }
}

/// The header and the thread, as the core last sent them.
@MainActor
final class ConversationThread: ObservableObject {
  @Published private(set) var shown = Muster_ConversationChanged()
  /// An answer asked to take the keyboard, for the thread to give it to; the thread clears it.
  @Published var answerFocus: AnswerFocus?

  var group: ConversationGroup? {
    shown.group.isEmpty ? nil : ConversationGroup(daemon: shown.daemonID, group: shown.group)
  }

  /// The latest question the human may answer, which Tab from the message box reaches; older
  /// ones take a click.
  var newestQuestion: UInt64? { newestQuestionMessage?.seq }

  /// Who asked it, for the message box to say whose answers Tab reaches.
  var newestQuestionAuthor: String? { newestQuestionMessage?.author }

  private var newestQuestionMessage: Muster_ConversationMessage? {
    for row in shown.rows.reversed() {
      if case .message(let message) = row.kind, message.answerable, message.answeredBy.isEmpty,
        !message.answers.isEmpty
      {
        return message
      }
    }
    return nil
  }

  /// Counts the times a group came on screen, so the thread opens each one on its unread line:
  /// the hosted view, and what it remembers, outlives leaving the conversation.
  private(set) var opening = 0

  func apply(_ changed: Muster_ConversationChanged) {
    let before = group
    shown = changed
    if let group, group != before { opening += 1 }
  }
}

/// The members column, and what each member's agent is doing.
///
/// What an agent is doing is its pane's own event, which arrives for every pane in the window;
/// only a member's is kept, so an agent elsewhere changing state redraws nothing here.
@MainActor
final class ConversationMembers: ObservableObject {
  @Published private(set) var members: [Muster_ConversationMember] = []
  @Published private(set) var agents: [PaneKey: PaneAgent] = [:]
  /// The roster's line for each member's pane: what its agent says it is doing.
  @Published private(set) var subtitles: [PaneKey: String] = [:]
  /// The keys that reach each member's pane, as the core numbered them for the agent list.
  @Published private(set) var chords: [PaneKey: Chord] = [:]
  /// The members' panes, which the sidebar marks.
  private(set) var panes: Set<PaneKey> = []

  struct Chord: Equatable {
    let first: Int
    let second: Int
  }

  func apply(
    _ changed: Muster_ConversationMembersChanged, agents: [PaneKey: PaneAgent], roster: Roster
  ) {
    members = changed.members
    panes = Set(members.compactMap(\.pane))
    self.agents = agents.filter { panes.contains($0.key) }
    apply(roster: roster)
  }

  func apply(pane: PaneKey, agent: PaneAgent) {
    guard panes.contains(pane), agents[pane] != agent else { return }
    agents[pane] = agent
  }

  func apply(roster: Roster) {
    let fresh = Dictionary(
      roster.panes.filter { panes.contains($0.key) }.map { ($0.key, $0.subtitle) },
      uniquingKeysWith: { first, _ in first })
    if fresh != subtitles { subtitles = fresh }
    let keys = Dictionary(
      roster.panes.filter { panes.contains($0.key) }.map {
        ($0.key, Chord(first: $0.tabPress, second: $0.press))
      }, uniquingKeysWith: { first, _ in first })
    if keys != chords { chords = keys }
  }
}

extension Muster_ConversationMember {
  /// The member's pane, for Go to pane and its agent's state, when it is in one this window
  /// reaches.
  var pane: PaneKey? { paneID.isEmpty ? nil : PaneKey(daemon: daemonID, pane: paneID) }
}

/// What a member's row says, worked out apart from drawing it.
enum MemberLine {
  /// The state its dot is painted for: its agent's, or what the daemon says of the member.
  static func state(_ member: Muster_ConversationMember, agent: PaneAgent?) -> String {
    switch member.liveness {
    case .unreachable: "unreachable"
    case .gone: "unknown"
    default: agent?.state ?? "unknown"
    }
  }

  /// What the member is doing: its agent's own line, as the agent list says it.
  static func doing(
    _ member: Muster_ConversationMember, agent: PaneAgent?, subtitle: String?
  ) -> String {
    switch member.liveness {
    case .human: return ""
    case .gone: return "no longer running"
    case .unreachable: return "on a machine that cannot be reached now"
    default: break
    }
    guard let agent else { return member.pane == nil ? "in no pane this window shows" : "" }
    return SidebarModel.subtitle(subtitle ?? "", agent: agent)
  }

  /// Whether messages wait on the member unread, from whom, and why it has not read them, in the
  /// daemon's words.
  static func unread(_ member: Muster_ConversationMember) -> String? {
    guard member.unreadFirst > 0 else { return nil }
    let from = member.unreadFrom.isEmpty ? "" : " from \(member.unreadFrom)"
    let why = member.unreadWhy.isEmpty ? "" : ": \(member.unreadWhy)"
    return "has not read what waits\(from)\(why)"
  }
}

/// The message box: the draft as the core keeps it, and where a post from it stands.
///
/// Every edit is the core's to take in (`SaveDraft`), and its answer replaces the box's own
/// text: a leading @name may have become a recipient, Backspace may have turned one back into
/// text. The field applies the answer to an edit it made itself; anything else that changes the
/// draft bumps `revision`, which the field takes as its cue to show the core's text.
@MainActor
final class MessageBox: ObservableObject {
  @Published private(set) var group: ConversationGroup?
  @Published private(set) var draft = Muster_MessageBoxDraft()
  @Published private(set) var sent = Muster_PostOutcome()
  @Published private var queuedPosts: Set<ConversationGroup> = []
  /// The completion Tab takes.
  @Published private(set) var highlighted = 0
  private(set) var revision = 0

  /// The chord that sends, as the core binds `send_message`.
  var sendChord: Core.Binding?
  var requests: ConversationRequests
  /// Puts the keyboard in the field, for Reply.
  var onFocus: () -> Void = {}
  /// Gives the keyboard to the newest question's answers, if there is one to answer.
  var tabToAnswers: () -> Bool = { false }
  /// What the field holds now, with its cursor in scalars; nil while it is composing or absent.
  var fieldContents: () -> (text: String, cursor: Int)? = { nil }

  init(requests: ConversationRequests) {
    self.requests = requests
  }

  var recipients: [String] { draft.to.map(\.name) }
  var pending: Bool { sent.state == .pending || group.map(queuedPosts.contains) == true }

  func apply(_ changed: Muster_MessageBoxChanged) {
    group =
      changed.group.isEmpty
      ? nil : ConversationGroup(daemon: changed.daemonID, group: changed.group)
    sent = changed.hasSent ? changed.sent : Muster_PostOutcome()
    take(changed.hasDraft ? changed.draft : Muster_MessageBoxDraft(), fromField: false)
  }

  /// The field's text changed; answers with the draft the field should show.
  @discardableResult
  func edited(text: String, cursor: Int) -> Muster_MessageBoxDraft? {
    save(edit(text: text, cursor: cursor), fromField: true)
  }

  /// The cursor moved without the text changing, so what it completes may have: the arrows or
  /// a click. Nothing is sent while the field's text differs from the core's draft, since that
  /// is an edit, which `edited` sends, and nothing when the cursor is already where the core
  /// has it, as after the field shows one of its answers.
  @discardableResult
  func moved(text: String, cursor: Int) -> Muster_MessageBoxDraft? {
    guard text == draft.text, cursor != Int(draft.cursor) else { return nil }
    return save(edit(text: text, cursor: cursor), fromField: true)
  }

  /// Backspace straight after the last recipient, which turns it back into text.
  @discardableResult
  func unaddress(text: String) -> Muster_MessageBoxDraft? {
    var unaddressing = edit(text: text, cursor: 0)
    unaddressing.unaddress = true
    return save(unaddressing, fromField: true)
  }

  /// Completes the @name at the cursor to the highlighted completion, from the field.
  @discardableResult
  func completeHighlighted(text: String, cursor: Int) -> Muster_MessageBoxDraft? {
    guard draft.completions.indices.contains(highlighted) else { return nil }
    return complete(
      draft.completions[highlighted].name, text: text, cursor: cursor, fromField: true)
  }

  /// Completes the @name at the cursor to `name`, chosen with the pointer.
  func complete(_ name: String) {
    complete(name, text: draft.text, cursor: Int(draft.cursor), fromField: false)
    onFocus()
  }

  func moveHighlight(by step: Int) {
    let count = draft.completions.count
    guard count > 0 else { return }
    highlighted = (highlighted + step + count) % count
  }

  func remove(recipient: String) {
    var removing = edit(text: draft.text, cursor: Int(draft.cursor))
    removing.to.removeAll { $0 == recipient }
    save(removing, fromField: false)
  }

  /// Reply on a message: addressed to its author, keeping what is written.
  func reply(to author: String) {
    var replying = edit(text: draft.text, cursor: Int(draft.cursor))
    replying.to = [author]
    save(replying, fromField: false)
    onFocus()
  }

  func setUrgent(_ urgent: Bool) {
    var marking = edit(text: draft.text, cursor: Int(draft.cursor))
    marking.urgent = urgent
    save(marking, fromField: false)
  }

  /// The answer picked from a question in the thread, to go with the words.
  var picked: (question: UInt64, answer: String)? {
    draft.pickedQuestion == 0 ? nil : (draft.pickedQuestion, draft.pickedAnswer)
  }

  /// Whether Send posts anything: the core's rule, which counts an answer picked with nothing
  /// written as a whole reply.
  var sendable: Bool { !pending && draft.sendable }

  /// Picks question #`question`'s answer at `answer`, which fills the reply rather than sending
  /// it (MIP-9, section 6), and puts the keyboard in the box for a note or the send chord
  /// (mine). Picking the picked answer again takes it back; the core says which.
  func pick(question: UInt64, answer: Int) {
    guard let group, !pending,
      let answered = requests.pick(group, question, UInt32(clamping: answer))
    else { return }
    take(answered, fromField: false)
    onFocus()
  }

  /// Takes the picked answer back: the cross beside it.
  func unpick() {
    guard !pending, let group, let picked, let answered = requests.pick(group, picked.question, nil)
    else { return }
    take(answered, fromField: false)
  }

  func send() {
    guard !pending else { return }
    // The core posts its own copy of the draft, which hears of the field's text on each change.
    // An input method that commits its composition without changing the text would leave that
    // copy a composition behind, so the field's words go first.
    if let field = fieldContents(), field.text != draft.text {
      edited(text: field.text, cursor: field.cursor)
    }
    guard let group, sendable else { return }
    Core.info("conversation.send", ["daemon": group.daemon, "group": group.group])
    queuedPosts.insert(group)
    requests.post(group) { [weak self] in self?.queuedPosts.remove(group) }
  }

  private func edit(text: String, cursor: Int) -> DraftEdit {
    DraftEdit(text: text, cursor: cursor, to: recipients, urgent: draft.urgent)
  }

  @discardableResult
  private func complete(
    _ name: String, text: String, cursor: Int, fromField: Bool
  ) -> Muster_MessageBoxDraft? {
    var completing = edit(text: text, cursor: cursor)
    completing.complete = name
    return save(completing, fromField: fromField)
  }

  @discardableResult
  private func save(_ edit: DraftEdit, fromField: Bool) -> Muster_MessageBoxDraft? {
    guard !pending, let group, let answer = requests.saveDraft(group, edit) else { return nil }
    take(answer, fromField: fromField)
    return answer
  }

  private func take(_ fresh: Muster_MessageBoxDraft, fromField: Bool) {
    if fresh.completions != draft.completions { highlighted = 0 }
    draft = fresh
    if !fromField { revision += 1 }
  }
}

/// What a key press means while a conversation is shown, wherever in it the keyboard is.
enum ConversationKeys {
  enum Meaning: Equatable {
    /// Back to the panes.
    case leave
    /// Ends the input method's composition in the message box, and nothing else (MIP-9,
    /// section 7): somebody typing Japanese presses Escape to drop a candidate, not to leave.
    case endComposition
    /// Sends the message box: the send chord pressed with the keyboard elsewhere in the
    /// conversation, on an answer's button say. The box's field matches it itself.
    case send
    /// Not the conversation's: on to whatever holds the keyboard.
    case pass
  }

  static func meaning(
    key: String, modifiers: NSEvent.ModifierFlags, leave chord: Core.Binding?, composing: Bool,
    send: Core.Binding? = nil, inField: Bool = false
  ) -> Meaning {
    if chord?.matches(key: key, modifiers: modifiers) == true {
      return composing ? .endComposition : .leave
    }
    if !inField, send?.matches(key: key, modifiers: modifiers) == true { return .send }
    return .pass
  }
}

extension Muster_ConversationRow {
  /// What the thread knows this row by, stable as rows arrive around it: a message by its number,
  /// a fold by its first message, a divider by its day.
  var identity: String {
    switch kind {
    case .divider(let date): "d:\(date)"
    case .changes(let changes): "c:\(changes.atMs)"
    case .fold(let fold): "f:\(fold.first)"
    case .message(let message): "m:\(message.seq)"
    case .unread: "unread"
    case nil: "none"
    }
  }
}

/// The rows with an identity each that no other row shares: two merged change rows from the same
/// millisecond would otherwise collide.
func identified(_ rows: [Muster_ConversationRow]) -> [(id: String, row: Muster_ConversationRow)] {
  var seen: [String: Int] = [:]
  return rows.map { row in
    let identity = row.identity
    let count = seen[identity, default: 0]
    seen[identity] = count + 1
    return (count == 0 ? identity : "\(identity)#\(count)", row)
  }
}

/// The conversation over one window's panes, and the only thing outside this file that knows one
/// exists. Built the first time the window shows a conversation, so a window that never shows one
/// does no conversation work.
@MainActor
final class ConversationHost {
  let thread = ConversationThread()
  let members = ConversationMembers()
  let box: MessageBox
  var requests: ConversationRequests {
    didSet { box.requests = requests }
  }
  private(set) var hosting: NSHostingView<ConversationView>?
  /// The group on screen, or nil while the window shows its panes.
  private(set) var shown: ConversationGroup?
  private var leaveChord: Core.Binding?
  /// Opens a link in a message, by the window's link policy.
  var onLink: (URL, ConversationGroup) -> Void = { _, _ in }

  init(requests: ConversationRequests) {
    self.requests = requests
    box = MessageBox(requests: requests)
    box.onFocus = { [weak self] in self?.takeKeyboard() }
    box.tabToAnswers = { [weak self] in self?.focusNewestQuestion() ?? false }
    box.fieldContents = { [weak self] in
      guard let hosting = self?.hosting, let field = Self.field(in: hosting),
        !field.hasMarkedText()
      else { return nil }
      return (field.string, field.scalarCursor)
    }
  }

  func take(bindings: [Core.Binding]) {
    leaveChord = bindings.first { $0.action == "leave_conversation" }
    box.sendChord = bindings.first { $0.action == "send_message" }
  }

  /// Shows `group`'s conversation over the panes, or takes it down for nil. Building the view
  /// the first time needs somewhere to put it, which `attach` is.
  func show(_ group: ConversationGroup?, attach: (NSView) -> Void) {
    let before = shown
    shown = group
    guard let group else {
      hosting?.isHidden = true
      return
    }
    let view = hosting ?? build(attach: attach)
    view.isHidden = false
    if before != group { takeKeyboard() }
  }

  private func build(attach: (NSView) -> Void) -> NSHostingView<ConversationView> {
    let view = NSHostingView(
      rootView: ConversationView(
        thread: thread, members: members, box: box, requests: { [weak self] in self?.requests },
        leaveChord: { [weak self] in self?.leaveChord },
        onLink: { [weak self] url, group in self?.onLink(url, group) }))
    attach(view)
    hosting = view
    return view
  }

  /// Puts the keyboard in the message box. Through AppKit, once SwiftUI has made the field: a
  /// view shown this turn has none yet.
  func takeKeyboard(attempts: Int = 5) {
    DispatchQueue.main.async { [weak self] in
      guard let self, shown != nil, let hosting, let window = hosting.window else { return }
      guard let field = Self.field(in: hosting) else {
        if attempts > 1 { takeKeyboard(attempts: attempts - 1) }
        return
      }
      if window.firstResponder !== field { window.makeFirstResponder(field) }
    }
  }

  /// The message box's text view, wherever SwiftUI put it.
  static func field(in view: NSView) -> MessageBoxTextView? {
    if let field = view as? MessageBoxTextView { return field }
    for child in view.subviews {
      if let field = field(in: child) { return field }
    }
    return nil
  }

  /// Whether the message box holds the keyboard.
  var holdsKeyboard: Bool {
    guard shown != nil, let hosting else { return false }
    return hosting.window?.firstResponder is MessageBoxTextView
  }

  /// Takes a key press the conversation means something by, before any view sees it: while the
  /// keyboard is somewhere in the conversation, or nowhere. A banner over it keeps its own keys.
  func handles(_ event: NSEvent) -> Bool {
    guard shown != nil, let hosting, let window = hosting.window else { return false }
    let responder = window.firstResponder
    if let view = responder as? NSView, !view.isDescendant(of: hosting) { return false }
    let field = responder as? NSTextView
    let composing = field?.hasMarkedText() ?? false
    switch ConversationKeys.meaning(
      key: event.charactersIgnoringModifiers ?? "", modifiers: event.modifierFlags,
      leave: leaveChord, composing: composing, send: box.sendChord,
      inField: responder is MessageBoxTextView)
    {
    case .leave:
      requests.leave()
      return true
    case .endComposition:
      return field?.endComposition() ?? false
    case .send:
      box.send()
      return true
    case .pass:
      return false
    }
  }

  /// Gives the keyboard to the first answer of the newest question the human may answer: Tab
  /// in the message box. False when there is none, and Tab is text.
  func focusNewestQuestion() -> Bool {
    guard let question = thread.newestQuestion else { return false }
    thread.answerFocus = AnswerFocus(question: question, index: 0)
    return true
  }
}

// MARK: - Drawing

struct ConversationView: View {
  let thread: ConversationThread
  let members: ConversationMembers
  let box: MessageBox
  /// Read when used rather than held, so the host can hand the view new ones.
  let requests: @MainActor () -> ConversationRequests?
  let leaveChord: @MainActor () -> Core.Binding?
  let onLink: @MainActor (URL, ConversationGroup) -> Void

  /// The terminal's colors, which the conversation is drawn in (`PaneLook`).
  @ObservedObject private var lookBoard = PaneLookBoard.shared

  var body: some View {
    VStack(spacing: 0) {
      ConversationHeader(thread: thread, requests: requests, leaveChord: leaveChord)
      PaneRule()
      HStack(spacing: 0) {
        VStack(spacing: 0) {
          ConversationThreadView(thread: thread, box: box, members: members, requests: requests)
          PaneRule()
          MessageBoxView(box: box, thread: thread)
        }
        PaneRule(vertical: true)
        ConversationMembersView(members: members, requests: requests)
          .frame(width: 220)
      }
    }
    .background(lookBoard.look.background)
    .inPaneLook(lookBoard)
    // Every link an agent writes, through the policy the terminal's links follow, rather than
    // opened as whatever scheme it names.
    .environment(
      \.openURL,
      OpenURLAction { url in
        if let group = thread.group { onLink(url, group) }
        return .handled
      })
  }
}

struct ConversationHeader: View {
  @ObservedObject var thread: ConversationThread
  let requests: @MainActor () -> ConversationRequests?
  let leaveChord: @MainActor () -> Core.Binding?

  @Environment(\.paneLook) private var look

  var body: some View {
    let header = thread.shown.header
    VStack(alignment: .leading, spacing: 4) {
      HStack(spacing: 8) {
        Text(header.group).font(.headline)
        if !header.keeper.isEmpty {
          Text("kept on \(header.keeper)").foregroundStyle(look.dimText)
        }
        if header.membersKnown {
          Text(header.members == 1 ? "1 member" : "\(header.members) members")
            .foregroundStyle(look.dimText)
        }
        Spacer()
        if let group = thread.group {
          Button(header.paused ? "Resume" : "Pause") {
            requests()?.pause(group, !header.paused)
          }
          .help(
            header.paused
              ? "Let posts here wake their members again" : "Have posts here wake nobody for now")
        }
        Button("Back to Panes") { requests()?.leave() }
          .help("Return to the panes (\(BannerReplyField.chord(leaveChord())))")
      }
      if !header.behind.isEmpty {
        Text(
          "\(header.behind) cannot be reached, so this group may be missing what was posted "
            + "there since, and a post here is refused until it answers."
        )
        .font(.callout).foregroundStyle(look.warning)
        .fixedSize(horizontal: false, vertical: true)
      }
      if header.paused {
        Text("Paused: a post here wakes nobody until the group resumes.")
          .font(.callout).foregroundStyle(look.dimText)
      }
      if let failed = Self.failure(thread.shown) {
        Text(failed)
          .font(.callout).foregroundStyle(look.alert)
          .fixedSize(horizontal: false, vertical: true)
      }
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 10)
  }
}

extension ConversationHeader {
  /// Why the conversation shows no more of the log, in the core's own sentence: it says whether
  /// Muster is still asking or the group is gone, and only it knows which.
  static func failure(_ shown: Muster_ConversationChanged) -> String? {
    shown.failed.isEmpty ? nil : shown.failed
  }
}

struct ConversationThreadView: View {
  @ObservedObject var thread: ConversationThread
  let box: MessageBox
  let members: ConversationMembers
  let requests: @MainActor () -> ConversationRequests?

  /// The opening whose first rows this thread has scrolled to, so it does that once each time a
  /// group comes on screen.
  @State private var scrolledFor: Int?
  /// Whether the end of the thread is on screen, which is when a new message scrolls to it.
  @State private var atEnd = true
  /// The row that was first when "Load earlier" was chosen, to stay on once the page lands.
  @State private var keeping: String?
  /// Which question's answer holds the keyboard, if one does.
  @FocusState private var answerFocus: AnswerFocus?

  var body: some View {
    let rows = identified(thread.shown.rows)
    ScrollViewReader { proxy in
      ScrollView {
        LazyVStack(alignment: .leading, spacing: 8) {
          if thread.shown.earlier, let group = thread.group {
            Button("Load earlier") {
              keeping = rows.first?.id
              requests()?.loadEarlier(group)
            }
            .buttonStyle(.link)
            .frame(maxWidth: .infinity)
          }
          if thread.shown.loading {
            ProgressView().controlSize(.small).frame(maxWidth: .infinity)
          }
          ForEach(rows, id: \.id) { entry in
            ConversationRowView(
              row: entry.row, group: thread.group, box: box, members: members,
              requests: requests, focus: $answerFocus
            )
            .id(entry.id)
          }
          Color.clear.frame(height: 1).id("end")
            .onAppear { atEnd = true }
            .onDisappear { atEnd = false }
        }
        .padding(14)
      }
      .onChange(of: rows.map(\.id)) { before, after in
        scroll(proxy, from: before, to: after)
      }
      // A group left and shown again can come back with the rows it had, which changes no id.
      .onChange(of: thread.opening) { _, _ in scroll(proxy, from: [], to: rows.map(\.id)) }
      .onChange(of: thread.answerFocus) { _, wanted in
        guard let wanted else { return }
        thread.answerFocus = nil
        proxy.scrollTo("m:\(wanted.question)")
        // A row scrolled to this turn may not be drawn yet, and an undrawn button cannot take
        // the keyboard.
        DispatchQueue.main.async { answerFocus = wanted }
      }
      .onAppear { scroll(proxy, from: [], to: rows.map(\.id)) }
    }
  }

  private func scroll(_ proxy: ScrollViewProxy, from before: [String], to after: [String]) {
    guard thread.group != nil else { return }
    let scrolling = ThreadScroll.next(
      opening: thread.opening, scrolledFor: scrolledFor, before: before, after: after,
      keeping: keeping, atEnd: atEnd)
    switch scrolling {
    case .open(let row):
      scrolledFor = thread.opening
      proxy.scrollTo(row, anchor: .top)
    case .keep(let row):
      keeping = nil
      proxy.scrollTo(row, anchor: .top)
    case .follow:
      proxy.scrollTo("end", anchor: .bottom)
    case nil:
      break
    }
  }
}

/// Where a conversation's thread scrolls when its rows change: on opening, to the first message
/// the human has not read, or the end; afterwards to new messages only while the end is on
/// screen, and to the reader's place when an earlier page lands above it (mine).
enum ThreadScroll: Equatable {
  /// The thread's first scroll since it came on screen, to this row's top.
  case open(String)
  /// Back to the row that was first when an earlier page was asked for.
  case keep(String)
  /// To the end, after a new message.
  case follow

  static func next(
    opening: Int, scrolledFor: Int?, before: [String], after: [String], keeping: String?,
    atEnd: Bool
  ) -> ThreadScroll? {
    guard !after.isEmpty else { return nil }
    if scrolledFor != opening {
      return .open(after.contains("unread") ? "unread" : "end")
    }
    if let keeping, before.first != after.first, after.contains(keeping) {
      return .keep(keeping)
    }
    // Rows that were not there before are the thread arriving, not a message: an opening that
    // changed the rows too is told twice, and the second telling would scroll past the unread
    // line to the end.
    if atEnd, !before.isEmpty, before.last != after.last {
      return .follow
    }
    return nil
  }
}

struct ConversationRowView: View {
  let row: Muster_ConversationRow
  let group: ConversationGroup?
  let box: MessageBox
  let members: ConversationMembers
  let requests: @MainActor () -> ConversationRequests?
  var focus: FocusState<AnswerFocus?>.Binding

  @Environment(\.paneLook) private var look

  var body: some View {
    switch row.kind {
    case .divider(let date):
      HStack(spacing: 8) {
        rule
        Text(date).font(.caption.monospacedDigit()).foregroundStyle(look.dimText)
        rule
      }
    case .changes(let changes):
      HStack(spacing: 6) {
        Text(changes.text)
        Spacer()
        time(changes.atMs, look: look)
      }
      .font(.caption)
      .foregroundStyle(look.dimText)
    case .fold(let fold):
      // Messages between agents, none to the person, as one line: a chevron, how many and when.
      Button(action: { if let group { requests()?.openFold(group, fold.first) } }) {
        HStack(spacing: 6) {
          Image(systemName: "chevron.right")
          Text("\(fold.count) messages between agents, none to you")
          Spacer()
          Text(
            "\(BannerTime.timeOfDay(atMs: fold.fromMs))-\(BannerTime.timeOfDay(atMs: fold.toMs))"
          )
          .monospacedDigit()
          .help("\(BannerTime.exact(atMs: fold.fromMs)) to \(BannerTime.exact(atMs: fold.toMs))")
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(RoundedRectangle(cornerRadius: 8).fill(look.raised))
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .font(.callout)
      .foregroundStyle(look.dimText)
      .help("Show these messages")
    case .message(let message):
      ConversationMessageView(
        message: message, group: group, box: box, members: members, requests: requests,
        focus: focus)
    case .unread:
      HStack(spacing: 8) {
        Rectangle().fill(look.alert).frame(height: 1)
        Text("Unread").font(.caption.bold()).foregroundStyle(look.alert)
        Rectangle().fill(look.alert).frame(height: 1)
      }
    case nil:
      EmptyView()
    }
  }

  private var rule: some View {
    Rectangle().fill(look.rule).frame(height: 1)
  }
}

/// A time in the thread: the time of day, under its day's divider, with the milliseconds and the
/// UTC offset when the pointer is on it.
@MainActor
private func time(_ atMs: UInt64, look: PaneLook) -> some View {
  Text(BannerTime.timeOfDay(atMs: atMs))
    .font(.caption)
    .monospacedDigit()
    .foregroundStyle(look.dimText)
    .help(BannerTime.exact(atMs: atMs))
}

/// A message's author as an avatar: their identity color and, for an agent this window shows, its
/// state. Observes the members alone, so an agent changing state redraws this and not the message.
struct AuthorAvatar: View {
  let message: Muster_ConversationMessage
  @ObservedObject var members: ConversationMembers

  var body: some View {
    let pane =
      message.authorPaneID.isEmpty
      ? nil : PaneKey(daemon: message.authorDaemonID, pane: message.authorPaneID)
    AvatarView(
      name: message.author,
      color: IdentityColor.color(
        paneID: message.authorPaneID, name: message.authorName, isHuman: message.byYou,
        isMuster: message.fromMuster),
      state: pane.flatMap { members.agents[$0]?.state })
  }
}

/// "Go to Pane", with the keys that reach the pane as the agent list draws them.
struct PaneLink: View {
  let pane: PaneKey
  let help: String
  @ObservedObject var members: ConversationMembers
  let requests: @MainActor () -> ConversationRequests?

  var body: some View {
    HStack(spacing: 4) {
      Button("Go to Pane") { requests()?.focus(pane) }
        .buttonStyle(.link)
        .help(help)
      if let chord = members.chords[pane] {
        ChordKeys(first: chord.first, second: chord.second)
      }
    }
  }
}

struct ConversationMessageView: View {
  let message: Muster_ConversationMessage
  let group: ConversationGroup?
  let box: MessageBox
  let members: ConversationMembers
  let requests: @MainActor () -> ConversationRequests?
  var focus: FocusState<AnswerFocus?>.Binding

  @Environment(\.paneLook) private var look

  var body: some View {
    HStack(alignment: .top, spacing: 8) {
      AuthorAvatar(message: message, members: members)
      VStack(alignment: .leading, spacing: 6) {
        HStack(spacing: 6) {
          Text(message.author).bold()
          if !message.to.isEmpty {
            Text("to \(message.to.joined(separator: ", "))").foregroundStyle(look.dimText)
          }
          if message.toYou {
            TagView(text: "To you", foreground: look.accent, fill: look.raised)
          }
          if message.urgent {
            TagView(text: "Urgent", foreground: look.alert, fill: look.raised)
          }
          Spacer()
          time(message.atMs, look: look)
        }
        MarkdownBlocksView(blocks: message.body)
          .textSelection(.enabled)
          .foregroundStyle(message.fromMuster ? look.dimText : look.text)
        if message.more, let group {
          Button("Show all") { requests()?.showWhole(group, message.seq) }
            .buttonStyle(.link)
        }
        if !message.answers.isEmpty {
          QuestionAnswersView(message: message, box: box, focus: focus)
        }
        if !message.said.isEmpty {
          Text(message.said).font(.caption).foregroundStyle(look.dimText)
        }
        actions
      }
    }
    .padding(10)
    .background(tint)
    .overlay(alignment: .leading) {
      // A message to the person has a bar down its edge, which a tint alone did not give.
      if message.toYou { Rectangle().fill(look.accent).frame(width: 3) }
    }
    .clipShape(RoundedRectangle(cornerRadius: 8))
  }

  @ViewBuilder private var actions: some View {
    let pane =
      message.authorPaneID.isEmpty
      ? nil : PaneKey(daemon: message.authorDaemonID, pane: message.authorPaneID)
    if message.replyable || pane != nil {
      HStack(spacing: 12) {
        if message.replyable {
          Button("Reply") { box.reply(to: message.authorName) }
            .buttonStyle(.link)
            .help("Address the message box to \(message.author)")
        }
        if let pane {
          PaneLink(
            pane: pane, help: "Return to the panes, on \(message.author)'s", members: members,
            requests: requests)
        }
      }
      .font(.callout)
    }
  }

  private var tint: Color {
    if message.toYou { return look.accentTint }
    if message.byYou { return look.raised }
    return .clear
  }
}

/// What holds the keyboard among the thread's answers: question #`question`'s answer at `index`.
struct AnswerFocus: Hashable {
  let question: UInt64
  let index: Int
}

/// A question's answers under it (MIP-9, section 6): buttons while the human may pick one, who
/// answered once somebody has, and otherwise - a question asked of somebody else - the answers
/// listed, so the human still sees what was offered (mine).
struct QuestionAnswersView: View {
  let message: Muster_ConversationMessage
  @ObservedObject var box: MessageBox
  var focus: FocusState<AnswerFocus?>.Binding

  @Environment(\.paneLook) private var look

  var body: some View {
    if !message.answeredBy.isEmpty {
      Text(
        message.answeredInWords
          ? "Answered by \(message.answeredBy) in words"
          : "Answered by \(message.answeredBy): \(message.answeredWith)"
      )
      .font(.callout)
      .foregroundStyle(look.dimText)
    } else if message.answerable {
      let question = message.seq
      AnswerButtons(
        answers: message.answers, picked: picked,
        recommended: message.hasRecommended ? Int(message.recommended) : nil,
        enabled: !box.pending, focus: focus,
        focusing: { AnswerFocus(question: question, index: $0) },
        onPick: { box.pick(question: question, answer: $0) },
        onTabOut: { _ in
          focus.wrappedValue = nil
          box.onFocus()
        })
    } else {
      VStack(alignment: .leading, spacing: 2) {
        ForEach(Array(message.answers.enumerated()), id: \.offset) { index, answer in
          HStack(spacing: 6) {
            Text("\(index + 1)").monospacedDigit().foregroundStyle(look.dimText)
            Text(answer).foregroundStyle(look.dimText)
            if message.hasRecommended, Int(message.recommended) == index { RecommendedLabel() }
          }
        }
      }
      .font(.callout)
    }
  }

  /// The answer the message box holds for this question, by its place.
  private var picked: Int? {
    guard let held = box.picked, held.question == message.seq else { return nil }
    return message.answers.firstIndex(of: held.answer)
  }
}

struct ConversationMembersView: View {
  @ObservedObject var members: ConversationMembers
  let requests: @MainActor () -> ConversationRequests?

  @Environment(\.paneLook) private var look

  var body: some View {
    ScrollView {
      VStack(alignment: .leading, spacing: 12) {
        Text("Members").font(.caption.bold()).foregroundStyle(look.dimText)
        ForEach(members.members, id: \.name) { member in
          memberRow(member)
        }
      }
      .padding(12)
      .frame(maxWidth: .infinity, alignment: .leading)
    }
  }

  private func memberRow(_ member: Muster_ConversationMember) -> some View {
    let agent = member.pane.flatMap { members.agents[$0] }
    let doing = MemberLine.doing(
      member, agent: agent, subtitle: member.pane.flatMap { members.subtitles[$0] })
    let isHuman = member.liveness == .human
    return HStack(alignment: .top, spacing: 8) {
      AvatarView(
        name: member.shown,
        color: IdentityColor.color(
          paneID: member.paneID, name: member.name, isHuman: isHuman,
          isMuster: member.name.drop(while: { $0 == "@" }) == "muster"),
        state: isHuman ? nil : MemberLine.state(member, agent: agent), size: 22)
      VStack(alignment: .leading, spacing: 2) {
        HStack(spacing: 6) {
          Text(member.shown).bold()
          Spacer(minLength: 0)
          if let pane = member.pane, let chord = members.chords[pane] {
            ChordKeys(first: chord.first, second: chord.second)
          }
        }
        if !doing.isEmpty {
          Text(doing).font(.caption).foregroundStyle(look.dimText)
            .fixedSize(horizontal: false, vertical: true)
        }
        if let unread = MemberLine.unread(member) {
          Text(unread).font(.caption).foregroundStyle(look.warning)
            .fixedSize(horizontal: false, vertical: true)
        }
        if let pane = member.pane {
          Button("Go to Pane") { requests()?.focus(pane) }
            .buttonStyle(.link)
            .font(.caption)
        }
      }
    }
  }
}

/// A hairline between regions, in the terminal's colors rather than the system's.
struct PaneRule: View {
  var vertical = false

  @Environment(\.paneLook) private var look

  var body: some View {
    Rectangle().fill(look.rule)
      .frame(width: vertical ? 1 : nil, height: vertical ? nil : 1)
  }
}

/// Says that Tab reaches a question's answers while one waits for the person.
struct AnswerHint: View {
  @ObservedObject var thread: ConversationThread

  @Environment(\.paneLook) private var look

  var body: some View {
    if let author = thread.newestQuestionAuthor {
      Text("Tab reaches \(author)'s answers").font(.caption).foregroundStyle(look.dimText)
    }
  }
}

struct MessageBoxView: View {
  @ObservedObject var box: MessageBox
  let thread: ConversationThread

  @Environment(\.paneLook) private var look

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      if !box.draft.completions.isEmpty {
        completions
      }
      VStack(alignment: .leading, spacing: 6) {
        recipients
        if let picked = box.picked {
          HStack(spacing: 4) {
            Text("Answer: \(picked.answer)").bold()
            Button(action: { box.unpick() }) { Image(systemName: "xmark.circle.fill") }
              .buttonStyle(.plain)
              .foregroundStyle(look.dimText)
              .disabled(box.pending)
              .help("Take the answer back")
          }
          .font(.callout)
        }
        MessageBoxField(box: box)
          .frame(height: 64)
      }
      .padding(8)
      .background(RoundedRectangle(cornerRadius: 8).fill(look.raised))
      .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(look.rule))
      HStack(spacing: 10) {
        Toggle("Urgent", isOn: Binding(get: { box.draft.urgent }, set: { box.setUrgent($0) }))
          .toggleStyle(.checkbox)
          .disabled(box.pending)
          .help("Reach an agent at work, mid-turn; the group's policy may refuse it")
        AnswerHint(thread: thread)
        Spacer()
        Button(action: { box.send() }) {
          HStack(spacing: 6) {
            Text("Send")
            ChordCap(text: BannerReplyField.chord(box.sendChord), onAccent: true)
          }
        }
        .buttonStyle(.borderedProminent)
        .disabled(!box.sendable)
        .help("Send (\(BannerReplyField.chord(box.sendChord)))")
      }
      outcome
    }
    .padding(10)
  }

  @ViewBuilder private var recipients: some View {
    if box.draft.to.isEmpty {
      Text("To nobody in particular: the group's policy decides whom this wakes")
        .font(.caption).foregroundStyle(look.dimText)
    } else {
      HStack(spacing: 6) {
        Text("To").font(.caption).foregroundStyle(look.dimText)
        ForEach(box.draft.to, id: \.name) { recipient in
          HStack(spacing: 2) {
            Text(recipient.shown)
            Button(action: { box.remove(recipient: recipient.name) }) {
              Image(systemName: "xmark.circle.fill")
            }
            .buttonStyle(.plain)
            .foregroundStyle(look.dimText)
            .disabled(box.pending)
            .help("Address the message to the group's policy instead of \(recipient.shown)")
          }
          .font(.callout)
          .padding(.horizontal, 6)
          .padding(.vertical, 2)
          .background(look.accentTint)
          .clipShape(Capsule())
        }
      }
    }
  }

  private var completions: some View {
    HStack(spacing: 6) {
      ForEach(Array(box.draft.completions.enumerated()), id: \.offset) { index, completion in
        Button("@\(completion.shown)") { box.complete(completion.name) }
          .buttonStyle(.plain)
          .padding(.horizontal, 6)
          .padding(.vertical, 2)
          .background(index == box.highlighted ? look.accentTint : .clear)
          .clipShape(RoundedRectangle(cornerRadius: 4))
      }
      Text("Tab to complete").font(.caption).foregroundStyle(look.dimText)
    }
    .font(.callout)
  }

  @ViewBuilder private var outcome: some View {
    switch box.pending ? Muster_PostOutcome.State.pending : box.sent.state {
    case .pending:
      Text("Sending…").font(.caption).foregroundStyle(look.dimText)
    case .posted:
      Text("Sent - \(box.sent.said)")
        .font(.caption)
        .foregroundStyle(box.sent.heard ? look.dimText : look.warning)
    case .refused:
      Text("Not sent: \(box.sent.said)")
        .font(.caption).foregroundStyle(look.alert)
        .fixedSize(horizontal: false, vertical: true)
    case .unknown:
      Text("Perhaps sent: \(box.sent.said)")
        .font(.caption).foregroundStyle(look.warning)
        .fixedSize(horizontal: false, vertical: true)
    default:
      EmptyView()
    }
  }
}
