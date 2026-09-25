import AppKit

/// The menu bar, which is where a macOS keybinding actually lives.
///
/// Not decoration, and not a second way to do things: on macOS a key equivalent is dispatched
/// from the main menu, so without an item here ⌘D is inert no matter what any view implements.
/// Putting splits here rather than matching chords in `keyDown` is what makes them the
/// platform's own keybindings - the user can rebind them in System Settings, they show the
/// shortcut they are bound to, and they keep working when that is not the one we assumed
/// (`README.md`, native feel).
///
/// The shortcuts are Ghostty's, deliberately. Somebody arriving from the terminal Muster
/// embeds should not have to learn a second set of them for the same actions.
@MainActor
public enum AppMenu {
  /// One item, as a table rather than as a call, so a test can read what was installed.
  public struct Item {
    public let title: String
    public let action: Selector
    public let key: String
    public let modifiers: NSEvent.ModifierFlags
    public let group: MenuActions.Group

    /// Which of several near-identical items this is, for the actions that come numbered.
    public let tag: Int

    /// The core's own name for the action, as `Action::as_str` spells it.
    ///
    /// Carried onto the item so that a dispatch can say which action fired without inverting
    /// the selector table - and so a rename in the core moves the log line with it rather than
    /// leaving a second spelling here to drift.
    public let name: String
  }

  /// What Muster does, as the core says it is bound.
  ///
  /// Built from the core rather than declared here, which is what makes rebinding one thing:
  /// a config file that moves `split_right` moves this item, and on macOS this item *is* the
  /// binding - a key equivalent on a menu item is how the platform decides what a chord means.
  ///
  /// An action with no chord is still an item. Somebody who unbound it did so to get the
  /// shortcut back, not to lose the action - and a menu is also where you look when you have
  /// forgotten what something is called.
  public static func paneItems(_ bindings: [Core.Binding]) -> [Item] {
    bindings.compactMap { binding in
      guard let described = MenuActions.byName[binding.action] else {
        // A core that names an action this shell has never heard of. Skipped rather than
        // guessed at, and said out loud: the symptom otherwise is a menu quietly missing a
        // line nobody can find.
        Core.warn(
          "menu.action.unknown",
          [
            "action": binding.action,
            "impact": "that action has no menu item and no shortcut; everything else in the "
              + "menu is unaffected",
            "check": "whether this shell is older than the core it is running against",
          ])
        return nil
      }
      return Item(
        title: described.title, action: described.selector,
        key: menuKeyEquivalent(forKeyNamed: binding.key) ?? "",
        modifiers: menuModifiers(binding.modifiers),
        group: described.group, tag: described.tag, name: binding.action)
    }
  }

  /// Builds the smallest menu bar that makes the platform's shortcuts work.
  ///
  /// An app with no menu at all is one a person cannot quit normally, and one whose ⌘V does
  /// nothing.
  ///
  /// Copy is here beside paste, and both go through the responder chain rather than through a
  /// chord matched in `keyDown`: that is how macOS decides what these mean, so a person who
  /// has rebound either gets what they bound. A pane's selection is the surface's own - made
  /// against the grid libghostty already painted - so neither needs a daemon to agree.
  public static func build(target: AnyObject, bindings: [Core.Binding]) -> NSMenu {
    let menu = LoggedMenu()

    let items = paneItems(bindings)

    let appItem = NSMenuItem()
    let appMenu = LoggedMenu()
    // Named for what it leaves behind rather than just "Quit muster". Quitting has always left
    // every session running, which is the founding promise and stays the default - and it was
    // invisible, so nobody could rely on it and nobody could ask for the other thing. The pair
    // is what makes the choice a choice (kan a_28YghIUw2).
    appMenu.addItem(
      withTitle: "Quit muster, Leaving Sessions Running",
      action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
    for item in items where item.group == .app {
      appMenu.addItem(entry(for: item, target: target))
    }
    appItem.submenu = appMenu
    menu.addItem(appItem)

    let editItem = NSMenuItem()
    let editMenu = LoggedMenu(title: "Edit")
    // nil target: AppKit walks the responder chain, which lands on the focused surface.
    editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
    editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
    editItem.submenu = editMenu
    menu.addItem(editItem)

    // One menu per group, in the order the groups are declared, and none for a group nothing
    // landed in. Splitting a pane and opening a list of shortcuts are not the same kind of
    // thing, and a single menu holding both is one nobody can scan.
    for group in MenuActions.Group.allCases where group != .app {
      let inGroup = items.filter { $0.group == group }
      if inGroup.isEmpty { continue }
      let groupItem = NSMenuItem()
      let groupMenu = LoggedMenu(title: group.rawValue)
      for item in inGroup {
        groupMenu.addItem(entry(for: item, target: target))
      }
      if group == .tab {
        groupMenu.addItem(MoveTabMenu.shared.item())
      }
      groupItem.submenu = groupMenu
      menu.addItem(groupItem)
    }

    return menu
  }

  /// One of Muster's actions as a menu item.
  ///
  /// An explicit target rather than the responder chain, because the first responder is a
  /// surface and these are not a surface's business. A chain walk would also make ⌘W mean
  /// "close the window" the moment no pane has focus, which is not what it says.
  ///
  /// The action's name rides along in `representedObject`, which is what tells `LoggedMenu`
  /// this item is one of Muster's and what to call it. Copy, Paste and the plain Quit item are
  /// built without one and write no record: they are the platform's, not the core's.
  private static func entry(for item: Item, target: AnyObject) -> NSMenuItem {
    let entry = NSMenuItem(title: item.title, action: item.action, keyEquivalent: item.key)
    entry.keyEquivalentModifierMask = item.modifiers
    entry.tag = item.tag
    entry.target = target
    entry.representedObject = item.name
    return entry
  }
}

/// A menu that writes down what was chosen before it dispatches it.
///
/// On macOS a key equivalent is dispatched from the menu, so this is the only place a bound
/// action's press exists at all: the keymap in the core never sees a chord Muster consumes, and
/// `input.key` records only keys that reach a pane. The log then had every effect and none of
/// the causes - eight `pane.font_size` lines 100 ms apart read as a replay bug for an evening
/// before a doc comment settled that somebody had held the chord (kan a_2KHGYh7xD).
///
/// Written before the dispatch rather than after, because the record is the cause and a cause
/// that lands after its effect is not one a reader can follow.
///
/// Every menu `AppMenu.build` makes is one of these, because AppKit dispatches a key equivalent
/// through the submenu holding the item rather than through the menu bar.
@MainActor
final class LoggedMenu: NSMenu {
  override func performActionForItem(at index: Int) {
    // `NSApp` is implicitly unwrapped and is nil until an application exists, so it is read as
    // the optional it is: a log line must not be the thing that ends the process. No event and
    // a menu pick are the same news anyway - nothing was pressed.
    let application: NSApplication? = NSApp
    if items.indices.contains(index),
      let fields = BoundAction.record(item: items[index], event: application?.currentEvent)
    {
      Core.debug(BoundAction.event, fields)
    }
    super.performActionForItem(at: index)
  }
}

/// What a bound action's press looks like in the run log.
///
/// Pure, so what a dispatch records can be asserted without a menu bar or a keyboard. The two
/// things there are to go on are the item that fired and the event AppKit was dispatching -
/// AppKit does not otherwise say how an item was picked.
enum BoundAction {
  static let event = "input.bound.action"

  /// The fields for one dispatch, or nothing when the item is not one of Muster's actions.
  ///
  /// `key` and `mods` are the two `input.bound.text` already carries, spelled by the same
  /// translation `input.key` goes through, so a reader who knows one record knows this one.
  /// They are empty rather than absent on a menu pick, so every line of this event has one
  /// shape. Nothing here is what was typed: a chord's name is not text, and the bytes an
  /// ordinary keystroke produces stay behind `MUSTER_LOG_INPUT` where they were.
  static func record(item: NSMenuItem, event: NSEvent?) -> [String: String]? {
    guard let name = item.representedObject as? String else { return nil }
    let pressed = chord(matching: event, for: item)
    return [
      "action": name,
      "key": pressed.map { KeyNames.name(forMacOSKeycode: UInt16($0.keyCode)) ?? "unidentified" }
        ?? "",
      "mods": pressed?.modifierFlags.musterNames.joined(separator: "+") ?? "",
      "repeat": pressed?.isARepeat == true ? "true" : "false",
      "source": pressed == nil ? "menu" : "chord",
    ]
  }

  /// The keystroke that chose this item, if a keystroke did.
  ///
  /// Matched against the item's own key equivalent rather than assumed from the event's type,
  /// because Return chooses a highlighted item too and its keyDown is not the item's chord. A
  /// person who rebound the shortcut in System Settings gets what they actually pressed here,
  /// which is the whole reason the chord is read off the event rather than off the binding.
  private static func chord(matching event: NSEvent?, for item: NSMenuItem) -> NSEvent? {
    guard let event, event.type == .keyDown, !item.keyEquivalent.isEmpty,
      event.charactersIgnoringModifiers?.lowercased() == item.keyEquivalent.lowercased(),
      event.modifierFlags.intersection(chordModifiers)
        == item.keyEquivalentModifierMask.intersection(chordModifiers)
    else { return nil }
    return event
  }

  /// The four a chord can carry. Caps lock and the side bits are reported in `mods` and never
  /// decide whether AppKit matched an item.
  private static let chordModifiers: NSEvent.ModifierFlags = [
    .command, .option, .control, .shift,
  ]
}

/// Move Tab to Window, which lists the other windows as they are when it opens.
///
/// Not one of the core's actions, because it has no chord to be bound to: a move needs a
/// destination, and a chord cannot name one. What it sends is the same `MoveTab` that `muster tab
/// move` and a dropped row send, so the three doors do one thing.
@MainActor
public final class MoveTabMenu: NSObject, NSMenuDelegate {
  public static let shared = MoveTabMenu()

  func item() -> NSMenuItem {
    let item = NSMenuItem(title: "Move Tab to Window", action: nil, keyEquivalent: "")
    let submenu = NSMenu(title: "Move Tab to Window")
    submenu.delegate = self
    item.submenu = submenu
    return item
  }

  /// Asked of the core each time, because windows open and close between one look and the next.
  public func menuNeedsUpdate(_ menu: NSMenu) {
    menu.removeAllItems()
    let windows = Core.otherWindows()
    if windows.isEmpty {
      let none = NSMenuItem(title: "No Other Windows", action: nil, keyEquivalent: "")
      none.isEnabled = false
      menu.addItem(none)
      return
    }
    for window in windows {
      let item = NSMenuItem(
        title: window.title, action: #selector(move(_:)), keyEquivalent: "")
      item.target = self
      item.representedObject = window.name
      menu.addItem(item)
    }
  }

  @objc private func move(_ sender: NSMenuItem) {
    guard let window = sender.representedObject as? String else { return }
    Core.info("tab.move.picked", ["window": window])
    Core.moveTab(to: window)
  }
}
