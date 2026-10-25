import AppKit
import SwiftUI

// The banners Muster draws for messages to the human (mip/0009-group-conversation.md, section 5).
// What stands, what each shows and what a reply came to are the core's (`banners.rs`); this file
// draws its last word and says what the person did. The second file in `Sources/` to import
// SwiftUI, on the terms `FindBar.swift` sets out: the hosting view's layer is forced clear, and
// the view is sized to its content so the panes around it keep their clicks.

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

  func send(_ banner: Muster_Banner) {
    let body = reply(for: banner)
    guard banner.replyable, !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
      return
    }
    Core.info(
      "banner.send", ["daemon": banner.daemonID, "group": banner.group, "seq": "\(banner.seq)"])
    core.reply(banner.daemonID, banner.group, banner.seq, body)
  }

  func dismiss(_ banner: Muster_Banner) {
    core.dismiss(banner.daemonID, banner.group, banner.seq)
  }

  /// Whether a key press is the chord that sends.
  func sends(key: String, modifiers: NSEvent.ModifierFlags) -> Bool {
    guard let chord = sendChord, !chord.key.isEmpty else { return false }
    let wanted = menuModifiers(chord.modifiers)
    let pressed = modifiers.intersection([.shift, .control, .option, .command])
    return menuKeyEquivalent(forKeyNamed: chord.key) == key && pressed == wanted
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
  var open: @MainActor (String, String) -> Void

  static let core = BannerRequests(
    reply: { Core.replyToBanner(daemonID: $0, group: $1, seq: $2, body: $3) },
    dismiss: { Core.dismissBanner(daemonID: $0, group: $1, seq: $2) },
    hold: { Core.holdBanner(daemonID: $0, group: $1, seq: $2, held: $3) },
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

  var body: some View {
    VStack(alignment: .trailing, spacing: 8) {
      ForEach(board.banners, id: \.identity) { banner in
        BannerCard(banner: banner, board: board, onLeave: onLeave)
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

struct BannerCard: View {
  let banner: Muster_Banner
  @ObservedObject var board: BannerBoard
  let onLeave: () -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      header
      content
      if !banner.waitingAuthor.isEmpty {
        Text("\(banner.waitingAuthor) wrote since\(banner.waitingUrgent ? " - urgent" : "")")
          .font(.caption)
          .foregroundStyle(banner.waitingUrgent ? .red : .secondary)
      }
      if banner.replyable || banner.sent.state == .refused || banner.sent.state == .unknown {
        BannerReplyField(banner: banner, board: board, onLeave: onLeave)
      }
      outcome
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
      Button(action: { board.dismiss(banner) }) { Image(systemName: "xmark") }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .help("Dismiss, leaving the message unread")
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

/// Where the human writes a reply. Sends on the core's `send_message` chord, matched here rather
/// than by the menu so the chord still reaches a pane when this field is not holding the keyboard.
struct BannerReplyField: View {
  let banner: Muster_Banner
  @ObservedObject var board: BannerBoard
  let onLeave: () -> Void
  @FocusState private var editing: Bool

  var body: some View {
    let text = Binding(get: { board.reply(for: banner) }, set: { board.setReply($0, for: banner) })
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
            Text("Reply to \(banner.author)")
              .foregroundStyle(.tertiary)
              .padding(.horizontal, 9)
              .padding(.vertical, 4)
              .allowsHitTesting(false)
          }
        }
        .onKeyPress(phases: .down) { press in
          let flags = BannerReplyField.flags(press.modifiers)
          guard board.sends(key: String(press.key.character), modifiers: flags) else {
            return .ignored
          }
          board.send(banner)
          return .handled
        }
        // An input method that is composing takes Esc first and ends the composition, so this
        // runs only for an Esc nothing else wanted.
        .onExitCommand { onLeave() }
        .disabled(banner.sent.state == .pending)
        .focused($editing)
        .onChange(of: editing) { _, editing in
          let key = BannerBoard.key(daemon: banner.daemonID, group: banner.group)
          if editing {
            board.focused = key
          } else if board.focused == key {
            board.focused = nil
          }
        }
      Button(action: { board.send(banner) }) { Image(systemName: "paperplane.fill") }
        .buttonStyle(.plain)
        .foregroundStyle(text.wrappedValue.isEmpty ? .tertiary : .primary)
        .disabled(!banner.replyable || text.wrappedValue.isEmpty)
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
      rootView: BannerStackView(board: board, onLeave: { [weak self] in self?.onLeave?() }))
    view.sizingOptions = [.intrinsicContentSize]
    view.layer?.backgroundColor = .clear
    return view
  }()

  /// Gives the keyboard back to the window's pane.
  var onLeave: (() -> Void)?

  init(board: BannerBoard = .shared) {
    self.board = board
  }

  func show(_ shown: Bool) {
    hosting.isHidden = !shown
  }

  /// Whether a reply field of this stack holds the keyboard. While it is being typed in, what
  /// AppKit calls first responder is a text view inside the hosting view.
  var holdsKeyboard: Bool {
    guard let responder = hosting.window?.firstResponder as? NSView else { return false }
    return responder.isDescendant(of: hosting)
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
