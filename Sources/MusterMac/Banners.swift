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
    let again = banner.hasPicked && Int(banner.picked) == index
    core.pick(banner.daemonID, banner.group, banner.seq, again ? nil : UInt32(index))
  }

  func unpick(_ banner: Muster_Banner) {
    core.pick(banner.daemonID, banner.group, banner.seq, nil)
  }

  /// A reply is its words, or an answer picked with nothing written, which is a whole reply.
  func send(_ banner: Muster_Banner) {
    let body = reply(for: banner)
    let written = !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    guard banner.replyable, written || banner.hasPicked else {
      return
    }
    Core.info(
      "banner.send", ["daemon": banner.daemonID, "group": banner.group, "seq": "\(banner.seq)"])
    core.reply(banner.daemonID, banner.group, banner.seq, body)
  }

  func dismiss(_ banner: Muster_Banner) {
    guard Self.dismissable(banner) else { return }
    core.dismiss(banner.daemonID, banner.group, banner.seq)
  }

  /// Whether the banner can be taken down: not while its reply is on its way, since the banner
  /// is where that reply's outcome shows, the human's words kept with a refused or unknown one.
  static func dismissable(_ banner: Muster_Banner) -> Bool {
    banner.sent.state != .pending
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
  var reply: @MainActor (String, String, UInt64, String) -> Void
  var dismiss: @MainActor (String, String, UInt64) -> Void
  var hold: @MainActor (String, String, UInt64, Bool) -> Void
  var pick: @MainActor (String, String, UInt64, UInt32?) -> Void
  var open: @MainActor (String, String) -> Void

  static let core = BannerRequests(
    reply: { Core.replyToBanner(daemonID: $0, group: $1, seq: $2, body: $3) },
    dismiss: { Core.dismissBanner(daemonID: $0, group: $1, seq: $2) },
    hold: { Core.holdBanner(daemonID: $0, group: $1, seq: $2, held: $3) },
    pick: { Core.pickBannerAnswer(daemonID: $0, group: $1, seq: $2, answer: $3) },
    open: { Core.openConversation(daemonID: $0, group: $1) })
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
                  .foregroundStyle(.secondary)
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
            .background(Color.primary.opacity(0.06))
            .clipShape(RoundedRectangle(cornerRadius: 4))
        case nil:
          EmptyView()
        }
      }
    }
  }
}

/// The stack: at most three banners, newest on top, and a count of the rest.
struct BannerStackView: View {
  @ObservedObject var board: BannerBoard
  /// Gives the keyboard back to the pane it came from.
  let onLeave: () -> Void
  /// Ends an input method's composition in the reply field holding the keyboard, if there is one.
  let endComposition: () -> Bool
  /// Opens a link in a banner's message, by the window's link policy, with its daemon and group.
  let onLink: (URL, String, String) -> Void

  var body: some View {
    VStack(alignment: .trailing, spacing: 8) {
      ForEach(board.banners, id: \.identity) { banner in
        BannerCard(
          banner: banner, board: board, onLeave: onLeave, endComposition: endComposition
        )
        .environment(
          \.openURL,
          OpenURLAction { url in
            onLink(url, banner.daemonID, banner.group)
            return .handled
          })
      }
      if board.folded > 0, let oldest = board.oldest {
        Button("\(board.folded) more - open the oldest") {
          board.core.open(oldest.daemonID, oldest.group)
        }
        .buttonStyle(.link)
        .padding(.horizontal, 8)
      }
    }
    .frame(width: 380)
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
  let onLeave: () -> Void
  let endComposition: () -> Bool
  @FocusState private var focus: BannerFocus?

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      header
      content
      if !banner.answers.isEmpty, banner.sent.state != .posted {
        BannerAnswers(banner: banner, board: board, focus: $focus, onLeave: onLeave)
      }
      if !banner.waitingAuthor.isEmpty {
        Text("\(banner.waitingAuthor) wrote since\(banner.waitingUrgent ? " - urgent" : "")")
          .font(.caption)
          .foregroundStyle(banner.waitingUrgent ? .red : .secondary)
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
    .padding(10)
    .background(.background)
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .shadow(radius: 4)
  }

  private var header: some View {
    HStack(spacing: 6) {
      Text(banner.group).bold()
      if !banner.author.isEmpty {
        Text(banner.author).foregroundStyle(.secondary)
      }
      if banner.urgent {
        Text("urgent").font(.caption.bold()).foregroundStyle(.red)
      }
      Spacer()
      if banner.atMs > 0 {
        Text(BannerTime.shown(atMs: banner.atMs))
          .font(.caption)
          .monospacedDigit()
          .foregroundStyle(.secondary)
          .help(BannerTime.exact(atMs: banner.atMs))
      }
      Button("Open") { board.core.open(banner.daemonID, banner.group) }
        .buttonStyle(.link)
        .help("Open the conversation")
      if BannerBoard.dismissable(banner) {
        Button(action: { board.dismiss(banner) }) { Image(systemName: "xmark") }
          .buttonStyle(.plain)
          .foregroundStyle(.secondary)
          .help("Dismiss, leaving the message unread")
      }
    }
  }

  @ViewBuilder private var content: some View {
    if !banner.unloaded.isEmpty {
      let from = banner.from.joined(separator: ", ")
      let count = banner.count == 1 ? "1 message" : "\(banner.count) messages"
      Text("\(count) for you\(from.isEmpty ? "" : " from \(from)"). \(banner.unloaded)")
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
    } else {
      MarkdownBlocksView(blocks: banner.body)
      if banner.more {
        Text("More in the conversation").font(.caption).foregroundStyle(.secondary)
      }
    }
  }

  @ViewBuilder private var outcome: some View {
    switch banner.sent.state {
    case .pending:
      Text("Sending…").font(.caption).foregroundStyle(.secondary)
    case .posted:
      Text("Sent - \(banner.sent.said)")
        .font(.caption)
        .foregroundStyle(banner.sent.heard ? Color.secondary : Color.orange)
    case .refused:
      Text("Not sent: \(banner.sent.said)")
        .font(.caption).foregroundStyle(.red)
        .fixedSize(horizontal: false, vertical: true)
    case .unknown:
      Text("Perhaps sent: \(banner.sent.said)")
        .font(.caption).foregroundStyle(.orange)
        .fixedSize(horizontal: false, vertical: true)
    default:
      EmptyView()
    }
  }
}

/// The answers a question offers, one button each (mip/0009-group-conversation.md, section 6).
/// Picking one fills the reply rather than sending it. Tab reaches them from the reply field
/// whatever the system's keyboard navigation setting, since a question is answered from the
/// keyboard as often as with the pointer; no digit picks one, since a reply may start with "1.".
struct BannerAnswers: View {
  let banner: Muster_Banner
  @ObservedObject var board: BannerBoard
  var focus: FocusState<BannerFocus?>.Binding
  let onLeave: () -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      ForEach(Array(banner.answers.enumerated()), id: \.offset) { index, answer in
        let picked = banner.hasPicked && Int(banner.picked) == index
        Button(action: { board.pick(index, in: banner) }) {
          HStack(spacing: 6) {
            Image(systemName: picked ? "checkmark.circle.fill" : "circle")
              .foregroundStyle(picked ? Color.accentColor : .secondary)
            Text(answer).frame(maxWidth: .infinity, alignment: .leading)
          }
          .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.vertical, 2)
        .focusable()
        .focused(focus, equals: .answer(index))
        .disabled(!banner.replyable)
        .help(picked ? "Take this answer back" : "Pick this answer; send it with or without a note")
        .onKeyPress(keys: [.space, .return], phases: .down) { _ in
          board.pick(index, in: banner)
          return .handled
        }
        .onKeyPress(keys: [.tab], phases: .down) { press in
          let back = press.modifiers.contains(.shift)
          let next = back ? index - 1 : index + 1
          focus.wrappedValue = banner.answers.indices.contains(next) ? .answer(next) : .reply
          return .handled
        }
        .onExitCommand { onLeave() }
      }
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
            .foregroundStyle(.secondary)
            .disabled(banner.sent.state == .pending)
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
        .background(Color.primary.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(alignment: .topLeading) {
          if text.wrappedValue.isEmpty {
            Text(
              picked == nil ? "Reply to \(banner.author)" : "Add a note, or send the answer alone"
            )
            .foregroundStyle(.tertiary)
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
        .disabled(banner.sent.state == .pending)
        .focused(focus, equals: .reply)
      let sendable = !text.wrappedValue.isEmpty || picked != nil
      Button(action: { board.send(banner) }) { Image(systemName: "paperplane.fill") }
        .buttonStyle(.plain)
        .foregroundStyle(sendable ? .primary : .tertiary)
        .disabled(!banner.replyable || !sendable)
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
        board: board, onLeave: { [weak self] in self?.onLeave?() },
        endComposition: { [weak self] in self?.endComposition() ?? false },
        onLink: { [weak self] url, daemon, group in self?.onLink?(url, daemon, group) }))
    view.sizingOptions = [.intrinsicContentSize]
    view.layer?.backgroundColor = .clear
    return view
  }()

  /// Gives the keyboard back to the window's pane.
  var onLeave: (() -> Void)?

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
