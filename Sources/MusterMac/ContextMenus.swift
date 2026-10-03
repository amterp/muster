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
/// actions or a copy of an id, so a right-click does nothing a chord or the CLI cannot.
public enum ContextMenuModel {
  /// A pane's menu, modeled on the one Ghostty's own surface opens.
  ///
  /// The machine submenu is left out while one machine is attached. It would list only the
  /// machine the pane is already on, and most windows never attach a second.
  public static func pane(machines: [String], hasSelection: Bool, canPaste: Bool)
    -> [ContextEntry]
  {
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
  public static func agentRow(onScreen: Bool) -> [ContextEntry] {
    [
      .item(.action("rename_pane")),
      .item(.action("move_pane_to_new_tab")),
      .separator,
      .item(.copyPaneID),
      .separator,
      .item(.action("close_pane"), enabled: onScreen),
    ]
  }
}

/// A right-click menu, built from `ContextMenuModel`'s entries.
///
/// A `LoggedMenu`, so a pick writes the same `input.bound.action` record a menu bar pick does.
/// It holds the objects its items target, because an `NSMenuItem` holds its target weakly and
/// nothing else would keep them alive while the menu is open.
@MainActor
final class ContextMenu: LoggedMenu {
  private var targets: [ContextMenuTarget] = []

  /// Titles come from `MenuActions` and chords from the core's bindings, so an item reads and
  /// shows the same chord as its menu bar twin. An action this shell has no title for is left
  /// out, as the menu bar leaves it out.
  static func build(
    _ entries: [ContextEntry], bindings: [Core.Binding], tab: String = "",
    perform: @escaping @MainActor (ContextChoice) -> Void
  ) -> ContextMenu {
    let menu = ContextMenu()
    // The model says what is enabled. Left on, AppKit would enable every item whose target
    // answers its selector, which is all of them.
    menu.autoenablesItems = false
    for entry in entries {
      switch entry {
      case .item(let choice, let enabled):
        guard let item = menu.item(for: choice, bindings: bindings, perform: perform) else {
          continue
        }
        item.isEnabled = enabled
        menu.addItem(item)
      case .submenu(let title, let inner):
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.submenu = build(inner, bindings: bindings, perform: perform)
        menu.addItem(item)
      case .moveTabToWindow:
        menu.addItem(MoveTabMenu.shared.item(tab: tab))
      case .separator:
        menu.addItem(.separator())
      }
    }
    return menu
  }

  private func item(
    for choice: ContextChoice, bindings: [Core.Binding],
    perform: @escaping @MainActor (ContextChoice) -> Void
  ) -> NSMenuItem? {
    guard let title = Self.title(for: choice) else { return nil }
    let target = ContextMenuTarget { perform(choice) }
    targets.append(target)
    let item = NSMenuItem(
      title: title, action: #selector(ContextMenuTarget.fire(_:)), keyEquivalent: "")
    item.target = target
    switch choice {
    case .action(let name):
      // What tells `LoggedMenu` this is one of the core's actions, and which.
      item.representedObject = name
      if let bound = bindings.first(where: { $0.action == name }) {
        item.keyEquivalent = menuKeyEquivalent(forKeyNamed: bound.key) ?? ""
        item.keyEquivalentModifierMask = menuModifiers(bound.modifiers)
      }
    case .copy:
      item.keyEquivalent = "c"
    case .paste:
      item.keyEquivalent = "v"
    default:
      break
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
    _ pane: PaneKey, surface: SurfaceView?, machines: [String], bindings: [Core.Binding],
    rename: @escaping @MainActor (PaneKey) -> Void
  ) -> NSMenu {
    let pasteboard = surface?.pasteboard ?? .general
    let entries = ContextMenuModel.pane(
      machines: machines, hasSelection: surface?.hasSelection ?? false,
      canPaste: surface?.canPaste ?? false)
    return ContextMenu.build(entries, bindings: bindings) { choice in
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
    _ pane: PaneKey, onScreen: Bool, bindings: [Core.Binding],
    pasteboard: NSPasteboard = .general, rename: @escaping @MainActor (PaneKey) -> Void
  ) -> NSMenu {
    ContextMenu.build(ContextMenuModel.agentRow(onScreen: onScreen), bindings: bindings) {
      perform($0, on: pane, pasteboard: pasteboard, rename: rename)
    }
  }

  /// A tab caption's menu. `firstPane` is where New Tab grows from: that pane's machine and
  /// directory, as cmd+T from inside the tab would use.
  public static func tab(
    _ tab: String, firstPane: PaneKey?, machines: [String], bindings: [Core.Binding],
    pasteboard: NSPasteboard = .general, rename: @escaping @MainActor (String) -> Void
  ) -> NSMenu {
    ContextMenu.build(ContextMenuModel.tab(machines: machines), bindings: bindings, tab: tab) {
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
