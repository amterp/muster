import AppKit

/// The Conversations menus: the way into a group's conversation now that the agent list has no
/// section listing them (Fencil `conversations-menus`).
///
/// Two places open one. A right-click on an agent's row or its pane lists the conversations the
/// person has with that agent, and the Window menu lists every group the person is in, which is
/// what keeps a group reachable once none of its agents has a pane. Each group is a submenu of its
/// own with Open Conversation, Leave Group and Delete Group, so nothing a group's row offered is
/// lost with the row.
public enum ConversationsMenu {
  /// One group as a menu lists it.
  public struct Entry: Equatable {
    public let daemon: String
    public let group: String
    public let title: String
  }

  /// The groups to list, in the core's order: every one the person is in, or with `pane` only
  /// those that are a conversation with that pane's agent.
  ///
  /// The core says who a group's other member is only for a group of two, so a pane's list is its
  /// one-to-one conversations. There can be more than one: a pair left over from an earlier pane
  /// with the same callsign stays a group of its own.
  ///
  /// A conversation with an agent this window has a pane for is titled with that agent's name
  /// when the whole list is drawn, where the group's own name (`@human+otter`) says less. In a
  /// pane's own list every entry is with the same agent, so the group's name is what tells two
  /// apart, and the whole list adds it after the agent's name for the same reason. A group on a
  /// machine other than the only one holding that name says which.
  public static func entries(
    groups: [SidebarModel.MessageGroup], roster: Roster, pane: PaneKey? = nil
  ) -> [Entry] {
    let listed = pane.map { pane in groups.filter { $0.partnerPane == pane } } ?? groups
    return listed.map { group in
      var title = group.group
      if pane == nil, let partner = group.partnerPane.flatMap(roster.pane) {
        title = partner.callsign.isEmpty ? partner.label : partner.callsign
        let shared = listed.contains {
          $0.partnerPane == group.partnerPane
            && ($0.group != group.group || $0.daemon != group.daemon)
        }
        if shared { title += " (\(group.group))" }
      }
      if listed.contains(where: { $0.group == group.group && $0.daemon != group.daemon }) {
        title += " on \(group.daemon)"
      }
      if group.unread > 0 { title += " (\(group.unread) unread)" }
      return Entry(daemon: group.daemon, group: group.group, title: title)
    }
  }

  /// What one group's submenu offers, top to bottom.
  public enum Choice: String, CaseIterable {
    case open = "Open Conversation"
    case leave = "Leave Group"
    case delete = "Delete Group…"
  }

  /// A Conversations item holding a submenu per entry, or nil when there is nothing to list.
  @MainActor
  static func item(
    _ entries: [Entry], perform: @escaping @MainActor (Choice, Entry) -> Void
  ) -> NSMenuItem? {
    guard !entries.isEmpty else { return nil }
    let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
    let menu = NSMenu(title: title)
    fill(menu, with: entries, perform: perform)
    item.submenu = menu
    return item
  }

  static let title = "Conversations"

  @MainActor
  static func fill(
    _ menu: NSMenu, with entries: [Entry], perform: @escaping @MainActor (Choice, Entry) -> Void
  ) {
    menu.removeAllItems()
    for entry in entries {
      let group = NSMenuItem(title: entry.title, action: nil, keyEquivalent: "")
      let choices = NSMenu(title: entry.title)
      for choice in Choice.allCases {
        // Leaving closes a group for the person alone; deleting ends it for every member.
        if choice == .delete { choices.addItem(.separator()) }
        let picked = ConversationsMenuItem(title: choice.rawValue) {
          Core.info(
            "conversation.menu.picked",
            ["choice": "\(choice)", "daemon": entry.daemon, "group": entry.group])
          perform(choice, entry)
        }
        choices.addItem(picked)
      }
      group.submenu = choices
      menu.addItem(group)
    }
  }
}

/// A menu item that runs a closure, and holds it: an `NSMenuItem` holds its target weakly.
@MainActor
final class ConversationsMenuItem: NSMenuItem {
  private let run: @MainActor () -> Void

  init(title: String, run: @escaping @MainActor () -> Void) {
    self.run = run
    super.init(title: title, action: #selector(fire(_:)), keyEquivalent: "")
    target = self
  }

  required init(coder: NSCoder) {
    fatalError("muster builds its menus in code")
  }

  @objc private func fire(_ sender: Any?) { run() }
}

/// The Window menu's Conversations submenu: every group the person is in, as it is when the menu
/// opens, acting for the window in front.
@MainActor
public final class WindowConversationsMenu: NSObject, NSMenuDelegate {
  public static let shared = WindowConversationsMenu()

  func item() -> NSMenuItem {
    let item = NSMenuItem(title: ConversationsMenu.title, action: nil, keyEquivalent: "")
    let submenu = NSMenu(title: ConversationsMenu.title)
    submenu.delegate = self
    item.submenu = submenu
    return item
  }

  /// Built each time, because groups are made, left and read between one look and the next.
  public func menuNeedsUpdate(_ menu: NSMenu) {
    let window = Windows.inFront as? MusterWindow
    let entries = window?.conversations() ?? []
    guard let window, !entries.isEmpty else {
      menu.removeAllItems()
      let none = NSMenuItem(title: "No Conversations", action: nil, keyEquivalent: "")
      none.isEnabled = false
      menu.addItem(none)
      return
    }
    ConversationsMenu.fill(menu, with: entries) { [weak window] choice, entry in
      window?.perform(choice, on: entry)
    }
  }
}
