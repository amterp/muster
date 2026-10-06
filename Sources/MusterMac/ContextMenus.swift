import AppKit

/// What one line of a right-click menu does, before it is an `NSMenuItem`.
public enum ContextChoice: Equatable, Sendable {
  /// One of the core's actions, by the name `Action::as_str` gives it, done to the menu's
  /// subject rather than to whatever has the keyboard.
  case action(String)
  case copy
  case paste
  case copyPaneID
  case copyTabID
  /// Split the pane right, starting the new pane on this machine.
  case splitOnto(machine: String)
  /// Make a tab on this machine.
  case newTabOn(machine: String)
  /// `muster msg leave --group G`, as the human.
  case leaveGroup
  /// `muster msg group delete G`, as the human.
  case deleteGroup
}

/// One line of a right-click menu.
public enum ContextEntry: Equatable, Sendable {
  case item(ContextChoice, enabled: Bool)
  case submenu(title: String, [ContextEntry])
  /// Move Tab to Window, which asks the core for the other windows when it opens.
  case moveTabToWindow
  case separator

  static func item(_ choice: ContextChoice) -> ContextEntry { .item(choice, enabled: true) }
}

/// What each right-click menu offers.
///
/// Pure, so what a menu holds can be asserted without a window. Every item is one of the core's
/// actions, a copy of an id, or a `muster msg` verb, so a right-click does nothing a chord or the
/// CLI cannot.
public enum ContextMenuModel {
  /// A pane's menu, modeled on the one Ghostty's own surface opens.
  ///
  /// The machine submenu is left out while one machine is attached. It would list only the
  /// machine the pane is already on, and most windows never attach a second.
  public static func pane(
    machines: [String], hasSelection: Bool, canPaste: Bool, canCompact: Bool = false
  ) -> [ContextEntry] {
    var entries: [ContextEntry] = [
      .item(.copy, enabled: hasSelection),
      .item(.paste, enabled: canPaste),
      .separator,
      .item(.action("split_right")),
      .item(.action("split_down")),
      .item(.action("split_left")),
      .item(.action("split_up")),
    ]
    if machines.count > 1 {
      entries.append(
        .submenu(title: "Split on Machine", machines.map { .item(.splitOnto(machine: $0)) }))
    }
    entries += [
      .separator,
      .item(.action("zoom")),
      .item(.action("move_pane_to_new_tab")),
      .item(.action("rename_pane")),
      .item(.action("compact_pane"), enabled: canCompact),
      .separator,
      .item(.copyPaneID),
      .separator,
      .item(.action("close_pane")),
    ]
    return entries
  }

  public static func tab(machines: [String]) -> [ContextEntry] {
    var entries: [ContextEntry] = [.item(.action("new_tab"))]
    if machines.count > 1 {
      entries.append(
        .submenu(title: "New Tab on Machine", machines.map { .item(.newTabOn(machine: $0)) }))
    }
    entries += [
      .separator,
      .item(.action("rename_tab")),
      .moveTabToWindow,
      .separator,
      .item(.copyTabID),
      .separator,
      .item(.action("close_tab")),
    ]
    return entries
  }

  /// No splitting or zooming, which act on a pane somebody is looking at, and the row is very
  /// often for one nobody is. Close is greyed out for such a pane because the core refuses to
  /// close a pane no region shows - the one destructive verb it keeps to what is on screen.
  ///
  /// Compact Agent is greyed out on both menus where the pane's daemon says nothing would be
  /// typed: no agent, or one whose manifest gives no compact line.
  public static func agentRow(onScreen: Bool, canCompact: Bool = false) -> [ContextEntry] {
    [
      .item(.action("rename_pane")),
      .item(.action("compact_pane"), enabled: canCompact),
      .item(.action("move_pane_to_new_tab")),
      .separator,
      .item(.copyPaneID),
      .separator,
      .item(.action("close_pane"), enabled: onScreen),
    ]
  }

  /// A message group's row. Leaving is what closes a group for the human, and takes it off the
  /// list; deleting ends it for everyone.
  public static func groupRow() -> [ContextEntry] {
    [.item(.leaveGroup), .separator, .item(.deleteGroup)]
  }
}

/// A right-click menu, built from `ContextMenuModel`'s entries.
///
/// It holds the objects its items target, because an `NSMenuItem` holds its target weakly and
/// nothing else would keep them alive while the menu is open.
@MainActor
final class ContextMenu: NSMenu {
  private var targets: [ContextMenuTarget] = []
  /// What was right-clicked, as the run log names it.
  private var subject: [String: String] = [:]
  /// The window it was opened in, which is the one its items act for: a right-click does not
  /// bring a window to the front, so the window in front may be another.
  private var window = ""

  /// Titles come from `MenuActions`, so an item reads as its menu bar twin does. An action this
  /// shell has no title for is left out, as the menu bar leaves it out.
  ///
  /// No item shows a chord. Each one acts on what was right-clicked and its chord acts on the
  /// pane with the keyboard, which is often another pane, so ⌘W beside Close Pane on an agent's
  /// row would say it closes that agent when it closes the one you are typing in. Finder and
  /// Xcode leave chords off their context menus for the same reason.
  static func build(
    _ entries: [ContextEntry], subject: [String: String], tab: String = "", window: String = "",
    perform: @escaping @MainActor (ContextChoice) -> Void
  ) -> ContextMenu {
    let menu = ContextMenu()
    menu.subject = subject
    menu.window = window
    // The model says what is enabled. Left on, AppKit would enable every item whose target
    // answers its selector, which is all of them.
    menu.autoenablesItems = false
    for entry in entries {
      switch entry {
      case .item(let choice, let enabled):
        guard let item = menu.item(for: choice, perform: perform) else { continue }
        item.isEnabled = enabled
        menu.addItem(item)
      case .submenu(let title, let inner):
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.submenu = build(inner, subject: subject, window: window, perform: perform)
        menu.addItem(item)
      case .moveTabToWindow:
        menu.addItem(MoveTabMenu.shared.item(tab: tab, window: window))
      case .separator:
        menu.addItem(.separator())
      }
    }
    return menu
  }

  /// Written before the dispatch, as `LoggedMenu` writes a menu bar pick, and told apart from
  /// one: the same action picked here was aimed at a pane or tab rather than at the keyboard's.
  override func performActionForItem(at index: Int) {
    if items.indices.contains(index),
      let fields = BoundAction.record(contextMenu: items[index], subject: subject)
    {
      Core.debug(BoundAction.event, fields)
    }
    Core.speaking(for: window) { super.performActionForItem(at: index) }
  }

  private func item(
    for choice: ContextChoice, perform: @escaping @MainActor (ContextChoice) -> Void
  ) -> NSMenuItem? {
    guard let title = Self.title(for: choice) else { return nil }
    let target = ContextMenuTarget { perform(choice) }
    targets.append(target)
    let item = NSMenuItem(
      title: title, action: #selector(ContextMenuTarget.fire(_:)), keyEquivalent: "")
    item.target = target
    if case .action(let name) = choice {
      // What tells the run log this is one of the core's actions, and which.
      item.representedObject = name
    }
    return item
  }

  static func title(for choice: ContextChoice) -> String? {
    switch choice {
    case .action(let name): MenuActions.byName[name]?.title
    case .copy: "Copy"
    case .paste: "Paste"
    case .copyPaneID: "Copy Pane ID"
    case .copyTabID: "Copy Tab ID"
    case .splitOnto(let machine), .newTabOn(let machine): machine
    case .leaveGroup: "Leave Group"
    case .deleteGroup: "Delete Group…"
    }
  }
}

@MainActor
final class ContextMenuTarget: NSObject {
  private let run: @MainActor () -> Void

  init(_ run: @escaping @MainActor () -> Void) {
    self.run = run
  }

  @objc func fire(_ sender: Any?) { run() }
}

/// The three menus, and what each of their items sends.
///
/// Every item names its subject - the pane or tab that was right-clicked - rather than leaving
/// the core to read "the one with the keyboard", so nothing depends on where the keyboard is by
/// the time somebody picks.
@MainActor
public enum ContextMenus {
  /// A pane's menu. `surface` is the one that was right-clicked, for Copy and Paste, and is nil
  /// only in a test that has none.
  public static func pane(
    _ pane: PaneKey, surface: SurfaceView?, machines: [String], canCompact: Bool = false,
    window: String = "", rename: @escaping @MainActor (PaneKey) -> Void
  ) -> NSMenu {
    let pasteboard = surface?.pasteboard ?? .general
    let entries = ContextMenuModel.pane(
      machines: machines, hasSelection: surface?.hasSelection ?? false,
      canPaste: surface?.canPaste ?? false, canCompact: canCompact)
    return ContextMenu.build(entries, subject: subject(pane), window: window) { choice in
      switch choice {
      case .copy: surface?.copy(nil)
      case .paste: surface?.paste(nil)
      case .splitOnto(let machine):
        Core.split(
          side: "right", daemonID: pane.daemon, paneID: pane.pane, newPaneDaemonID: machine)
      default: perform(choice, on: pane, pasteboard: pasteboard, rename: rename)
      }
    }
  }

  public static func agentRow(
    _ pane: PaneKey, onScreen: Bool, canCompact: Bool = false,
    pasteboard: NSPasteboard = .general, window: String = "",
    rename: @escaping @MainActor (PaneKey) -> Void
  ) -> NSMenu {
    ContextMenu.build(
      ContextMenuModel.agentRow(onScreen: onScreen, canCompact: canCompact),
      subject: subject(pane), window: window
    ) {
      perform($0, on: pane, pasteboard: pasteboard, rename: rename)
    }
  }

  /// A tab caption's menu. `firstPane` is where New Tab grows from: that pane's machine and
  /// directory, as cmd+T from inside the tab would use.
  public static func tab(
    _ tab: String, firstPane: PaneKey?, machines: [String], pasteboard: NSPasteboard = .general,
    window: String = "", rename: @escaping @MainActor (String) -> Void
  ) -> NSMenu {
    ContextMenu.build(
      ContextMenuModel.tab(machines: machines), subject: ["tab": tab], tab: tab, window: window
    ) {
      choice in
      switch choice {
      case .action("new_tab"):
        Core.createTab(daemonID: firstPane?.daemon ?? "", paneID: firstPane?.pane ?? "")
      case .action("rename_tab"): rename(tab)
      case .action("close_tab"): Core.closeTab(tabID: tab)
      case .newTabOn(let machine): Core.createTab(daemonID: machine)
      case .copyTabID: copy(tab, "tab", to: pasteboard)
      default: unhandled(choice, menu: "tab")
      }
    }
  }

  /// A message group row's menu. `delete` is called rather than the group deleted outright,
  /// because a delete takes every member's log with it and the window asks first.
  public static func groupRow(
    daemon: String, group: String, window: String = "",
    delete: @escaping @MainActor () -> Void
  ) -> NSMenu {
    ContextMenu.build(
      ContextMenuModel.groupRow(), subject: ["daemon": daemon, "group": group], window: window
    ) { choice in
      switch choice {
      case .leaveGroup:
        if !Core.leaveGroup(daemonID: daemon, group: group) { NSSound.beep() }
      case .deleteGroup: delete()
      default: unhandled(choice, menu: "group")
      }
    }
  }

  private static func subject(_ pane: PaneKey) -> [String: String] {
    ["daemon": pane.daemon, "pane": pane.pane]
  }

  private static func perform(
    _ choice: ContextChoice, on pane: PaneKey, pasteboard: NSPasteboard,
    rename: @MainActor (PaneKey) -> Void
  ) {
    switch choice {
    case .action(let name) where name.hasPrefix("split_"):
      Core.split(
        side: String(name.dropFirst("split_".count)), daemonID: pane.daemon,
        paneID: pane.pane)
    case .action("zoom"): Core.zoom(daemonID: pane.daemon, paneID: pane.pane)
    case .action("move_pane_to_new_tab"): Core.movePaneToNewTab(pane)
    case .action("rename_pane"): rename(pane)
    case .action("compact_pane"): Core.compactPane(daemonID: pane.daemon, paneID: pane.pane)
    case .action("close_pane"): Core.closePane(daemonID: pane.daemon, paneID: pane.pane)
    case .copyPaneID: copy(pane.pane, "pane", to: pasteboard)
    default: unhandled(choice, menu: "pane")
    }
  }

  /// Puts an id on the clipboard, which is the id `muster` takes for it - `--pane`, `--to`,
  /// `--tab` - so a menu pick and `muster window` hand over the same thing.
  private static func copy(_ id: String, _ kind: String, to pasteboard: NSPasteboard) {
    pasteboard.clearContents()
    pasteboard.setString(id, forType: .string)
    Core.debug("menu.id.copied", ["kind": kind, "id": id])
  }

  private static func unhandled(_ choice: ContextChoice, menu: String) {
    Core.warn(
      "menu.context.unhandled",
      [
        "choice": String(describing: choice),
        "menu": menu,
        "impact": "the item did nothing",
        "check": "this is a bug: ContextMenuModel offered an item ContextMenus has no request "
          + "for",
      ])
  }
}
