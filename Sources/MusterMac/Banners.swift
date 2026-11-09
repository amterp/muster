import AppKit
import SwiftUI

// The banners Muster draws for messages to the human (mip/0009-group-conversation.md, section 5).
// What stands, what each shows and what a reply came to are the core's (`banners.rs`); this file
// draws its last word and says what the person did. Hosted in SwiftUI on the terms
// `FindBar.swift` sets out: the hosting view's layer is forced clear, and the view is sized to
// its content so the panes around it keep their clicks.

/// The banners the core last sent, and the reply being written in each.
///
/// One for the app: the stack is about every window's messages and is drawn in the window in
/// front, so a reply half-written in one window is still there when another comes forward.
@MainActor
final class BannerBoard: ObservableObject {
  static let shared = BannerBoard()

  @Published private(set) var banners: [Muster_Banner] = []
  @Published private(set) var folded = 0
  @Published private(set) var oldest: Muster_Banner?

  /// What is written in each banner's reply field, by `key(daemon:group:)`.
  @Published private(set) var replies: [String: String] = [:]
  @Published private var queuedReplies: Set<String> = []

  /// The chord that sends, as the core binds `send_message`.
  private(set) var sendChord: Core.Binding?

  /// The banner whose reply field holds the keyboard, by `key(daemon:group:)`.
  @Published var focused: String?

  /// Takes the core's bindings, which say which chord sends.
  func take(bindings: [Core.Binding]) {
    sendChord = bindings.first { $0.action == "send_message" }
  }

  /// How long a reply that woke somebody stays up saying so before the banner comes down.
  var postedLinger: Duration = .seconds(4)

  /// The requests a banner sends, through the core unless a test says otherwise.
  var core: BannerRequests = .core

  static func key(daemon: String, group: String) -> String { "\(daemon)/\(group)" }

  func apply(_ changed: Muster_BannersChanged) {
    let before = Dictionary(
      banners.map { (Self.key(daemon: $0.daemonID, group: $0.group), $0) },
      uniquingKeysWith: { a, _ in a })
    banners = changed.shown
    folded = Int(changed.folded)
    if changed.oldestGroup.isEmpty {
      oldest = nil
    } else {
      var oldest = Muster_Banner()
      oldest.daemonID = changed.oldestDaemonID
      oldest.group = changed.oldestGroup
      self.oldest = oldest
    }
    for banner in changed.shown {
      let key = Self.key(daemon: banner.daemonID, group: banner.group)
      let was = before[key]
      if was?.seq != banner.seq {
        core.shown(banner.daemonID, banner.group)
      }
      switch banner.sent.state {
      case .posted:
        replies[key] = nil
        if banner.sent.heard, was?.sent.state != .posted || was?.seq != banner.seq {
          linger(banner)
        }
      case .refused, .unknown:
        // The words stay the human's: a field emptied while the reply was pending gets back
        // what the core kept.
        if was?.sent.state != banner.sent.state, (replies[key] ?? "").isEmpty {
          replies[key] = banner.sent.body
        }
      default:
        // A banner that moved on to a newer message starts with an empty field.
        if let was, was.seq != banner.seq, !banner.held { replies[key] = nil }
      }
    }
    // A banner holding a reply is always among those shown, so a reply for one that is not has
    // nobody to go to.
    let standing = Set(changed.shown.map { Self.key(daemon: $0.daemonID, group: $0.group) })
    for key in replies.keys where !standing.contains(key) {
      replies[key] = nil
    }
  }

  func reply(for banner: Muster_Banner) -> String {
    replies[Self.key(daemon: banner.daemonID, group: banner.group)] ?? ""
  }

  /// Takes what is written in a banner's reply field, and tells the core when the field starts
  /// or stops holding anything, which is what keeps the banner on the message being answered.
  func setReply(_ text: String, for banner: Muster_Banner) {
    guard !pending(banner) else { return }
    let key = Self.key(daemon: banner.daemonID, group: banner.group)
    let held = !(replies[key] ?? "").isEmpty
    replies[key] = text
    if held != !text.isEmpty {
      core.hold(banner.daemonID, banner.group, banner.seq, !text.isEmpty)
    }
  }

  /// Picks the answer at `index` of the banner's question, or takes the pick back when it is
  /// the one already picked. What is picked is the core's to keep, and comes back in the banner.
  func pick(_ index: Int, in banner: Muster_Banner) {
    guard !pending(banner) else { return }
    core.pick(banner.daemonID, banner.group, banner.seq, UInt32(index))
  }

  func unpick(_ banner: Muster_Banner) {
    guard !pending(banner) else { return }
    core.pick(banner.daemonID, banner.group, banner.seq, nil)
  }

  func send(_ banner: Muster_Banner) {
    let body = reply(for: banner)
    guard !pending(banner), banner.replyable, Self.canSend(text: body, picked: banner.hasPicked)
    else {
      return
    }
    Core.info(
      "banner.send", ["daemon": banner.daemonID, "group": banner.group, "seq": "\(banner.seq)"])
    let identity = banner.identity
    queuedReplies.insert(identity)
    core.reply(banner.daemonID, banner.group, banner.seq, body) { [weak self] in
      self?.queuedReplies.remove(identity)
    }
  }

  /// Whether a banner has to stay in full: its reply is being written or the keyboard is in it, or
  /// it is showing what a reply came to.
  func keeps(_ banner: Muster_Banner) -> Bool {
    let key = Self.key(daemon: banner.daemonID, group: banner.group)
    return banner.held || focused == key || !(replies[key] ?? "").isEmpty
      || banner.sent.state != .none || pending(banner)
  }

  func dismiss(_ banner: Muster_Banner) {
    guard dismissable(banner) else { return }
    core.dismiss(banner.daemonID, banner.group, banner.seq)
  }

  /// Whether a reply would send anything: words that are not all blank, or an answer picked
  /// with nothing written, which is a whole reply.
  nonisolated static func canSend(text: String, picked: Bool) -> Bool {
    picked || !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }

  /// Whether the banner can be taken down: not while its reply is on its way, since the banner
  /// is where that reply's outcome shows, the human's words kept with a refused or unknown one.
  func dismissable(_ banner: Muster_Banner) -> Bool {
    !pending(banner)
  }

  func pending(_ banner: Muster_Banner) -> Bool {
    queuedReplies.contains(banner.identity) || banner.sent.state == .pending
  }

  /// Whether a key press is the chord that sends.
  func sends(key: String, modifiers: NSEvent.ModifierFlags) -> Bool {
    sendChord?.matches(key: key, modifiers: modifiers) ?? false
  }

  /// A reply that woke somebody says so for a moment and comes down; one nobody live heard
  /// stays, since that is worth the human's attention.
  private func linger(_ banner: Muster_Banner) {
    let (daemon, group, seq, wait) = (banner.daemonID, banner.group, banner.seq, postedLinger)
    Task { @MainActor [weak self] in
      try? await Task.sleep(for: wait)
      guard let self,
        let still = banners.first(where: { $0.daemonID == daemon && $0.group == group }),
        still.seq == seq, still.sent.state == .posted
      else { return }
      core.dismiss(daemon, group, seq)
    }
  }
}

/// What a banner asks of the core, as functions so a test can see them without one.
struct BannerRequests {
  var reply:
    @MainActor (String, String, UInt64, String, _ then: @escaping @MainActor @Sendable () -> Void)
      -> Void
  var dismiss: @MainActor (String, String, UInt64) -> Void
  var hold: @MainActor (String, String, UInt64, Bool) -> Void
  var pick: @MainActor (String, String, UInt64, UInt32?) -> Void
  var open: @MainActor (String, String) -> Void
  /// A banner came up on a message it was not showing.
  var shown: @MainActor (String, String) -> Void

  static let core = BannerRequests(
    reply: { Core.replyToBanner(daemonID: $0, group: $1, seq: $2, body: $3, then: $4) },
    dismiss: { Core.dismissBanner(daemonID: $0, group: $1, seq: $2) },
    hold: { Core.holdBanner(daemonID: $0, group: $1, seq: $2, held: $3) },
    pick: { Core.pickBannerAnswer(daemonID: $0, group: $1, seq: $2, answer: $3) },
    open: { Core.openConversation(daemonID: $0, group: $1) },
    shown: { daemon, group in
      // Only drawn while a window draws banners; otherwise the system's notification says it.
      guard MessageNotification.bannerOnScreen else { return }
      Core.shown(daemonID: daemon, group: group, as: .banner, why: "message")
    })
}

/// When a message was posted, as a banner says it: the time of day with seconds, the date in
/// front when it is not today, and the milliseconds and UTC offset when the pointer is on it.
enum BannerTime {
  static func shown(atMs: UInt64, now: Date = Date(), zone: TimeZone = .current) -> String {
    let date = Date(timeIntervalSince1970: TimeInterval(atMs) / 1000)
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = zone
    let time = format(date, "HH:mm:ss", zone)
    return calendar.isDate(date, inSameDayAs: now)
      ? time : "\(format(date, "yyyy-MM-dd", zone)) \(time)"
  }

  /// The time of day alone, for a thread whose dividers say the day.
  static func timeOfDay(atMs: UInt64, zone: TimeZone = .current) -> String {
    format(Date(timeIntervalSince1970: TimeInterval(atMs) / 1000), "HH:mm:ss", zone)
  }

  static func exact(atMs: UInt64, zone: TimeZone = .current) -> String {
    format(
      Date(timeIntervalSince1970: TimeInterval(atMs) / 1000), "yyyy-MM-dd HH:mm:ss.SSS xxx", zone)
  }

  private static func format(_ date: Date, _ pattern: String, _ zone: TimeZone) -> String {
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.timeZone = zone
    formatter.dateFormat = pattern
    return formatter.string(from: date)
  }
}

/// A message's Markdown, as the core parsed it, in SwiftUI's terms.
enum BannerMarkdown {
  static func attributed(_ runs: [Muster_MarkdownRun]) -> AttributedString {
    var text = AttributedString()
    for run in runs {
      var piece = AttributedString(run.text)
      var intent: InlinePresentationIntent = []
      if run.emphasis { intent.insert(.emphasized) }
      if run.strong { intent.insert(.stronglyEmphasized) }
      if run.code { intent.insert(.code) }
      if !intent.isEmpty { piece.inlinePresentationIntent = intent }
      if !run.link.isEmpty, let url = URL(string: run.link) { piece.link = url }
      text.append(piece)
    }
    return text
  }
}

struct MarkdownBlocksView: View {
  let blocks: [Muster_MarkdownBlock]

  @Environment(\.paneLook) private var look

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
        switch block.kind {
        case .paragraph(let paragraph):
          Text(BannerMarkdown.attributed(paragraph.runs))
            .fixedSize(horizontal: false, vertical: true)
        case .list(let list):
          VStack(alignment: .leading, spacing: 2) {
            ForEach(Array(list.items.enumerated()), id: \.offset) { index, item in
              HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(list.numbered ? "\(list.start + UInt64(index))." : "•")
                  .foregroundStyle(look.dimText)
                MarkdownBlocksView(blocks: item.blocks)
              }
            }
          }
        case .code(let code):
          Text(code.text)
            .font(.system(.body, design: .monospaced))
            .textSelection(.enabled)
            .padding(6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(look.raised)
            .clipShape(RoundedRectangle(cornerRadius: 4))
        case nil:
          EmptyView()
        }
      }
    }
  }
}

/// Which banners the stack draws in full and which it folds into one line.
///
/// The first is drawn in full and the rest fold, so a burst of messages is one line rather than a
/// column of cards over the panes. A banner the human is writing a reply in, or that holds the
/// keyboard, or that shows what a reply came to, is never folded: the core keeps such a banner among
/// those shown for that reason, and folding it would hide the words being written.
enum BannerStackModel {
  struct Layout: Equatable {
    var full: [Muster_Banner]
    var folded: [Muster_Banner]
    /// Banners the core holds back beyond the ones it shows.
    var others: Int

    /// "2 more, from cactus and lynx", or nil when nothing is folded.
    var summary: String? {
      let count = folded.count + others
      guard count > 0 else { return nil }
      var names: [String] = []
      for banner in folded {
        let name = banner.author.isEmpty ? banner.group : banner.author
        if !names.contains(name) { names.append(name) }
      }
      var from = ""
      switch (names.count, others) {
      case (0, _): break
      case (1, 0): from = names[0]
      case (_, 0): from = names.dropLast().joined(separator: ", ") + " and " + names.last!
      default:
        from = names.joined(separator: ", ") + " and \(others) other\(others == 1 ? "" : "s")"
      }
      return from.isEmpty ? "\(count) more" : "\(count) more, from \(from)"
    }
  }

  static func layout(
    banners: [Muster_Banner], others: Int, keeps: (Muster_Banner) -> Bool, expanded: Bool
  ) -> Layout {
    guard !expanded else { return Layout(full: banners, folded: [], others: others) }
    var full: [Muster_Banner] = []
    var folded: [Muster_Banner] = []
    for (index, banner) in banners.enumerated() {
      if index == 0 || keeps(banner) { full.append(banner) } else { folded.append(banner) }
    }
    return Layout(full: full, folded: folded, others: others)
  }
}

/// Which pane an author's callsign is in this window, and what its agent is doing, so a banner's
/// avatar is the one the conversation draws for the same agent.
struct BannerAuthor: Equatable {
  let paneID: String
  let state: String?

  /// The one pane in `roster` whose callsign is `callsign`; nil when none is, or when two are,
  /// since a banner cannot say which of two was meant and the name's own color is the safer
  /// answer than another agent's.
  static func find(callsign: String, roster: Roster, agents: [PaneKey: PaneAgent]) -> Self? {
    guard !callsign.isEmpty else { return nil }
    let named = roster.panes.filter { $0.callsign == callsign }
    guard named.count == 1, let pane = named.first else { return nil }
    return BannerAuthor(paneID: pane.key.pane, state: agents[pane.key]?.state)
  }
}

/// The stack: the first banner in full, and one line for the rest.
struct BannerStackView: View {
  @ObservedObject var board: BannerBoard
  /// Finds an author by callsign among this window's panes.
  let identify: (String) -> BannerAuthor?
  @State private var expanded = false
  /// Gives the keyboard back to the pane it came from.
  let onLeave: () -> Void
  /// Ends an input method's composition in the reply field holding the keyboard, if there is one.
  let endComposition: () -> Bool
  /// Opens a link in a banner's message, by the window's link policy, with its daemon and group.
  let onLink: (URL, String, String) -> Void

  var body: some View {
    let layout = BannerStackModel.layout(
      banners: board.banners, others: board.folded, keeps: board.keeps, expanded: expanded)
    VStack(alignment: .trailing, spacing: 8) {
      ForEach(layout.full, id: \.identity) { banner in
        BannerCard(
          banner: banner, board: board, author: identify(banner.author),
          onLeave: onLeave, endComposition: endComposition
        )
        .environment(
          \.openURL,
          OpenURLAction { url in
            onLink(url, banner.daemonID, banner.group)
            return .handled
          })
      }
      if let summary = layout.summary {
        FoldedBannersLine(
          summary: summary,
          show: {
            if !layout.folded.isEmpty {
              expanded = true
            } else if let oldest = board.oldest {
              board.core.open(oldest.daemonID, oldest.group)
            }
          })
      }
    }
    .frame(width: 380)
    .inPaneLook()
  }

}

/// The banners past the first as one line. Show draws them in full; with none of them shown, it
/// opens the oldest conversation, as the count of unshown ones always did.
struct FoldedBannersLine: View {
  let summary: String
  let show: () -> Void

  @Environment(\.paneLook) private var look

  var body: some View {
    HStack(spacing: 8) {
      Text(summary).foregroundStyle(look.dimText).lineLimit(1)
      Spacer(minLength: 0)
      Button("Show", action: show).buttonStyle(.link)
    }
    .font(.callout)
    .padding(.horizontal, 10)
    .padding(.vertical, 6)
    .frame(width: 380)
    .background(look.background)
    .overlay(alignment: .leading) { Rectangle().fill(look.accent).frame(width: 3) }
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .shadow(radius: 3)
  }
}

/// What holds the keyboard inside a banner: its reply field, or one of its question's answers.
enum BannerFocus: Hashable {
  case reply
  case answer(Int)
}

struct BannerCard: View {
  let banner: Muster_Banner
  @ObservedObject var board: BannerBoard
  /// The author's pane in this window, when the callsign names one.
  var author: BannerAuthor?
  let onLeave: () -> Void
  let endComposition: () -> Bool
  @FocusState private var focus: BannerFocus?
  @Environment(\.paneLook) private var look

  var body: some View {
    HStack(alignment: .top, spacing: 8) {
      AvatarView(
        name: shownAuthor,
        color: IdentityColor.color(
          paneID: author?.paneID ?? "", name: banner.author,
          isMuster: banner.author.drop(while: { $0 == "@" }) == "muster"),
        state: author?.state)
      content
    }
    .padding(10)
    .background(look.background)
    .overlay(alignment: .leading) {
      // A banner is a message to the person, which the conversation marks with the same bar.
      Rectangle().fill(look.accent).frame(width: 3)
    }
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .shadow(radius: 4)
  }

  private var shownAuthor: String { banner.author.isEmpty ? banner.group : banner.author }

  private var content: some View {
    VStack(alignment: .leading, spacing: 6) {
      header
      messageBody
      if !banner.answers.isEmpty, banner.sent.state != .posted {
        AnswerButtons(
          answers: banner.answers, picked: banner.hasPicked ? Int(banner.picked) : nil,
          recommended: banner.hasRecommended ? Int(banner.recommended) : nil,
          enabled: banner.replyable && !board.pending(banner), focus: $focus,
          focusing: { .answer($0) },
          onPick: { board.pick($0, in: banner) }, onTabOut: { _ in focus = .reply }
        )
        .onExitCommand { onLeave() }
      }
      if !banner.waitingAuthor.isEmpty {
        Text("\(banner.waitingAuthor) wrote since\(banner.waitingUrgent ? " - urgent" : "")")
          .font(.caption)
          .foregroundStyle(banner.waitingUrgent ? look.alert : look.dimText)
      }
      if banner.replyable || banner.sent.state == .refused || banner.sent.state == .unknown {
        BannerReplyField(
          banner: banner, board: board, focus: $focus, onLeave: onLeave,
          endComposition: endComposition)
      }
      outcome
    }
    .onChange(of: focus) { _, focus in
      // The menu's Send Message sends from whichever banner holds the keyboard, answer or field.
      let key = BannerBoard.key(daemon: banner.daemonID, group: banner.group)
      if focus != nil {
        board.focused = key
      } else if board.focused == key {
        board.focused = nil
      }
    }
  }

  private var header: some View {
    HStack(spacing: 6) {
      // Who wrote it, then which conversation: "linen in human+linen".
      Text(shownAuthor).bold()
      if !banner.author.isEmpty {
        Text("in \(banner.group)").foregroundStyle(look.dimText).lineLimit(1)
      }
      TagView(text: "To you", foreground: look.accent, fill: look.raised)
      if banner.urgent {
        TagView(text: "Urgent", foreground: look.alert, fill: look.raised)
      }
      Spacer()
      if banner.atMs > 0 {
        Text(BannerTime.shown(atMs: banner.atMs))
          .font(.caption)
          .monospacedDigit()
          .foregroundStyle(look.dimText)
          .lineLimit(1)
          .fixedSize()
          .help(BannerTime.exact(atMs: banner.atMs))
      }
      Button("Open") { board.core.open(banner.daemonID, banner.group) }
        .buttonStyle(.link)
        .help("Open the conversation")
      if board.dismissable(banner) {
        Button(action: { board.dismiss(banner) }) { Image(systemName: "xmark") }
          .buttonStyle(.plain)
          .foregroundStyle(look.dimText)
          .help("Dismiss, leaving the message unread")
      }
    }
  }

  @ViewBuilder private var messageBody: some View {
    if !banner.unloaded.isEmpty {
      let from = banner.from.joined(separator: ", ")
      let count = banner.count == 1 ? "1 message" : "\(banner.count) messages"
      Text("\(count) for you\(from.isEmpty ? "" : " from \(from)"). \(banner.unloaded)")
        .foregroundStyle(look.dimText)
        .fixedSize(horizontal: false, vertical: true)
    } else {
      MarkdownBlocksView(blocks: banner.body)
      if banner.more {
        Text("More in the conversation").font(.caption).foregroundStyle(look.dimText)
      }
    }
  }

  @ViewBuilder private var outcome: some View {
    switch board.pending(banner) ? Muster_PostOutcome.State.pending : banner.sent.state {
    case .pending:
      Text("Sending…").font(.caption).foregroundStyle(look.dimText)
    case .posted:
      Text("Sent - \(banner.sent.said)")
        .font(.caption)
        .foregroundStyle(banner.sent.heard ? look.dimText : look.warning)
    case .refused:
      Text("Not sent: \(banner.sent.said)")
        .font(.caption).foregroundStyle(look.alert)
        .fixedSize(horizontal: false, vertical: true)
    case .unknown:
      Text("Perhaps sent: \(banner.sent.said)")
        .font(.caption).foregroundStyle(look.warning)
        .fixedSize(horizontal: false, vertical: true)
    default:
      EmptyView()
    }
  }
}

/// Where the human writes a reply. Sends on the core's `send_message` chord, matched here rather
/// than by the menu so the chord still reaches a pane when this field is not holding the keyboard.
struct BannerReplyField: View {
  let banner: Muster_Banner
  @ObservedObject var board: BannerBoard
  var focus: FocusState<BannerFocus?>.Binding
  let onLeave: () -> Void
  let endComposition: () -> Bool

  @Environment(\.paneLook) private var look

  /// The answer picked from the banner's question, shown beside what is written.
  private var picked: String? {
    guard banner.hasPicked, banner.answers.indices.contains(Int(banner.picked)) else { return nil }
    return banner.answers[Int(banner.picked)]
  }

  var body: some View {
    let text = Binding(get: { board.reply(for: banner) }, set: { board.setReply($0, for: banner) })
    VStack(alignment: .leading, spacing: 4) {
      if let picked {
        HStack(spacing: 4) {
          Text("Answer: \(picked)").bold()
          Button(action: { board.unpick(banner) }) { Image(systemName: "xmark.circle.fill") }
            .buttonStyle(.plain)
            .foregroundStyle(look.dimText)
            .disabled(board.pending(banner))
            .help("Take the answer back")
        }
        .font(.callout)
      }
      field(text)
    }
  }

  private func field(_ text: Binding<String>) -> some View {
    HStack(alignment: .bottom, spacing: 6) {
      TextEditor(text: text)
        .font(.body)
        .scrollContentBackground(.hidden)
        .frame(minHeight: 22, maxHeight: 88)
        .fixedSize(horizontal: false, vertical: true)
        .padding(4)
        .background(look.raised)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(look.rule))
        .overlay(alignment: .topLeading) {
          if text.wrappedValue.isEmpty {
            Text(
              picked == nil ? "Reply to \(banner.author)" : "Add a note, or send the answer alone"
            )
            .foregroundStyle(look.dimText)
            .padding(.horizontal, 9)
            .padding(.vertical, 4)
            .allowsHitTesting(false)
          }
        }
        .onKeyPress(phases: .down) { press in
          // Esc while composing ends the composition and nothing else (MIP-9, section 7). Done
          // here rather than left to the input method, since one that does not take Esc - a
          // dead key's accent waiting for its letter, or none at all - would strand it.
          if press.key == .escape, endComposition() { return .handled }
          if press.key == .tab, !press.modifiers.contains(.shift), !banner.answers.isEmpty,
            banner.replyable
          {
            focus.wrappedValue = .answer(0)
            return .handled
          }
          let flags = BannerReplyField.flags(press.modifiers)
          guard board.sends(key: String(press.key.character), modifiers: flags) else {
            return .ignored
          }
          board.send(banner)
          return .handled
        }
        // Only an Esc with nothing being composed reaches this: the key press above took the
        // others.
        .onExitCommand { onLeave() }
        .disabled(board.pending(banner))
        .focused(focus, equals: .reply)
      let sendable = BannerBoard.canSend(text: text.wrappedValue, picked: picked != nil)
      // The chord that sends, on a cap, is the send button: it says what to press and can be
      // clicked.
      Button(action: { board.send(banner) }) {
        ChordCap(text: BannerReplyField.chord(board.sendChord))
          .opacity(sendable ? 1 : 0.5)
      }
      .buttonStyle(.plain)
      .disabled(board.pending(banner) || !banner.replyable || !sendable)
      .help("Send (\(BannerReplyField.chord(board.sendChord)))")
    }
  }

  static func flags(_ modifiers: EventModifiers) -> NSEvent.ModifierFlags {
    var flags: NSEvent.ModifierFlags = []
    if modifiers.contains(.shift) { flags.insert(.shift) }
    if modifiers.contains(.control) { flags.insert(.control) }
    if modifiers.contains(.option) { flags.insert(.option) }
    if modifiers.contains(.command) { flags.insert(.command) }
    return flags
  }

  static func chord(_ binding: Core.Binding?) -> String {
    guard let binding else { return "unbound" }
    let spelled = Shortcuts.spell(binding)
    return spelled.isEmpty ? "unbound" : spelled
  }
}

extension Muster_Banner {
  /// Which group's banner this is, and which message it shows: a banner moving on to a newer
  /// message is drawn afresh, field and all.
  var identity: String { "\(daemonID)/\(group)#\(seq)" }
}

/// The stack over one window's panes, and the only thing outside this file that knows one
/// exists. Shown only while its window is the main one.
@MainActor
public final class BannerHost {
  let board: BannerBoard
  private(set) lazy var hosting: NSHostingView<BannerStackView> = {
    let view = NSHostingView(
      rootView: BannerStackView(
        board: board, identify: { [weak self] in self?.identify($0) },
        onLeave: { [weak self] in self?.onLeave?() },
        endComposition: { [weak self] in self?.endComposition() ?? false },
        onLink: { [weak self] url, daemon, group in self?.onLink?(url, daemon, group) }))
    view.sizingOptions = [.intrinsicContentSize]
    view.layer?.backgroundColor = .clear
    return view
  }()

  /// Gives the keyboard back to the window's pane.
  var onLeave: (() -> Void)?

  /// Finds the pane an author's callsign names among this window's, for the avatar's color and
  /// state.
  var identify: (String) -> BannerAuthor? = { _ in nil }

  /// Opens a link in a banner's message, with its daemon and group, by the window's link policy.
  var onLink: ((URL, String, String) -> Void)?

  init(board: BannerBoard = .shared) {
    self.board = board
  }

  func show(_ shown: Bool) {
    hosting.isHidden = !shown
  }

  var isShown: Bool { !hosting.isHidden }

  /// Whether a reply field of this stack holds the keyboard. While it is being typed in, what
  /// AppKit calls first responder is a text view inside the hosting view.
  var holdsKeyboard: Bool {
    guard let responder = hosting.window?.firstResponder as? NSView else { return false }
    return responder.isDescendant(of: hosting)
  }

  /// Ends an input method's composition in the reply field holding the keyboard, discarding what
  /// was being composed, as Esc does in a composing input method: told first, so one that keeps a
  /// composition of its own drops it, and then the field's marked text replaced with nothing in
  /// case it kept none. False when nothing was being composed.
  func endComposition() -> Bool {
    guard let field = hosting.window?.firstResponder as? NSTextView, field.isDescendant(of: hosting)
    else { return false }
    return field.endComposition()
  }

  /// Sends the reply in the banner whose field holds the keyboard: the menu's Send Message.
  func sendFocused() {
    guard holdsKeyboard,
      let banner = board.banners.first(where: {
        BannerBoard.key(daemon: $0.daemonID, group: $0.group) == board.focused
      })
    else { return }
    board.send(banner)
  }
}
