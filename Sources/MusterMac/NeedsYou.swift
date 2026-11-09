import Foundation

/// What the Needs you strip draws, worked out from the core's asking list and what the shell
/// already holds.
///
/// The core decides who is in the list and in what order (`Attention::asking()`, the same list
/// `focus --asking` walks), and this never reorders it: a person told "someone needs you" and a
/// chord that goes to someone must agree on who. An entry carries keys rather than labels, so the
/// words come from the roster, the agents and the groups, which the list already joins by key for
/// every row - and a renamed pane is redrawn without a new list.
public enum NeedsYouModel {
  /// One card: an agent or a group that wants the person.
  public struct Card: Equatable {
    public enum Subject: Equatable {
      case pane(PaneKey)
      case group(daemon: String, group: String)
    }

    public let subject: Subject
    /// Why it is asking, as the core spells it: `blocked`, `stuck`, `done`, `notified` or `message`.
    public let alert: String
    /// The state its icon is drawn for, or empty on a group.
    public let state: String
    /// Its callsign, or the group's name as the Messages list shows it.
    public let title: String
    /// What it wants, in words, with how long it has waited where that is known: `Blocked 1m`,
    /// `2 to you`.
    public let word: String
    /// What it said: a blocked agent's subtitle, or the start of the latest message to the person.
    public let detail: String
    /// The presses that reach a pane, in order; none on a group or on a pane no chord reaches.
    public let keys: [Int]
    /// Whether the word carries a time, which is whether a minute passing changes it.
    public let isTimed: Bool

    /// Whether the card is tinted and carries a colored edge, as the rows of the same states are.
    public var isTinted: Bool { alert == "blocked" || alert == "stuck" }

    public var isGroup: Bool {
      if case .group = subject { return true }
      return false
    }
  }

  /// The cards for the asking list, in the order the core sent it.
  ///
  /// A pane the roster does not hold is still asking, so it still gets a card, named by its id: a
  /// strip that dropped an entry would disagree with the chord. The list is computed at `now`,
  /// which only the time held depends on.
  public static func cards(
    entries: [AskingEntry], roster: Roster, agents: [PaneKey: PaneAgent],
    groups: [SidebarModel.MessageGroup], now: Date
  ) -> [Card] {
    entries.map { entry in
      let held = SidebarModel.heldFor(sinceMS: entry.sinceMs, now: now)
      let timed = { (word: String) in held.isEmpty ? word : "\(word) \(held)" }
      guard entry.group.isEmpty else {
        let group = groups.first { $0.daemon == entry.daemon && $0.group == entry.group }
        return Card(
          subject: .group(daemon: entry.daemon, group: entry.group), alert: entry.alert,
          state: "",
          title: OneToOne.name(
            daemon: entry.daemon, group: entry.group, groups: groups, roster: roster)
            ?? entry.group,
          word: groupWord(group), detail: entry.preview, keys: [],
          isTimed: false)
      }
      let key = PaneKey(daemon: entry.daemon, pane: entry.pane)
      let pane = roster.pane(key)
      let agent = agents[key] ?? "unknown"
      let word =
        switch entry.alert {
        case "blocked": timed("Blocked")
        case "stuck": timed("Stuck")
        case "done": timed("Done")
        case "notified": timed("Notified")
        default: timed(entry.alert.prefix(1).uppercased() + entry.alert.dropFirst())
        }
      return Card(
        subject: .pane(key), alert: entry.alert, state: agent.state,
        title: pane.map { $0.callsign.isEmpty ? $0.label : $0.callsign } ?? entry.pane, word: word,
        // What a blocked agent asked is a card of its own; until it lands, its row's subtitle.
        detail: entry.alert == "blocked"
          ? pane.map { SidebarModel.subtitle($0.subtitle, agent: agent) } ?? "" : "",
        keys: pane.map { [$0.tabPress, $0.press].filter(SidebarRowView.pressable) } ?? [],
        isTimed: !held.isEmpty)
    }
  }

  /// What a group's card says it wants: the messages addressed to the person, else those unread.
  private static func groupWord(_ group: SidebarModel.MessageGroup?) -> String {
    guard let group else { return "Message" }
    if group.toYou > 0 { return "\(group.toYou) to you" }
    return group.unread > 0 ? "\(group.unread) unread" : "Message"
  }

  /// Whether some card shows how long it has waited.
  public static func showsTime(_ cards: [Card]) -> Bool {
    cards.contains(where: \.isTimed)
  }

  /// What a card says on hover and to VoiceOver, one fact a line.
  public static func details(of card: Card) -> String {
    [card.title, card.word, card.detail].filter { !$0.isEmpty }.joined(separator: "\n")
  }
}
