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
        group: described.group)
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
    let menu = NSMenu()

    let appItem = NSMenuItem()
    let appMenu = NSMenu()
    appMenu.addItem(
      withTitle: "Quit muster", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
    appItem.submenu = appMenu
    menu.addItem(appItem)

    let editItem = NSMenuItem()
    let editMenu = NSMenu(title: "Edit")
    // nil target: AppKit walks the responder chain, which lands on the focused surface.
    editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
    editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
    editItem.submenu = editMenu
    menu.addItem(editItem)

    // One menu per group, in the order the groups are declared, and none for a group nothing
    // landed in. Splitting a pane and opening a list of shortcuts are not the same kind of
    // thing, and a single menu holding both is one nobody can scan.
    let items = paneItems(bindings)
    for group in MenuActions.Group.allCases {
      let inGroup = items.filter { $0.group == group }
      if inGroup.isEmpty { continue }
      let groupItem = NSMenuItem()
      let groupMenu = NSMenu(title: group.rawValue)
      for item in inGroup {
        // An explicit target rather than the responder chain, because the first responder is
        // a surface and these are not a surface's business. A chain walk would also make ⌘W
        // mean "close the window" the moment no pane has focus, which is not what it says.
        let entry = NSMenuItem(title: item.title, action: item.action, keyEquivalent: item.key)
        entry.keyEquivalentModifierMask = item.modifiers
        entry.target = target
        groupMenu.addItem(entry)
      }
      groupItem.submenu = groupMenu
      menu.addItem(groupItem)
    }

    return menu
  }
}
