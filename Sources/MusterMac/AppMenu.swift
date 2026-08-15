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
  }

  /// What Muster does to panes. Every one goes to the core; none of them changes a window
  /// directly.
  @MainActor public static let paneItems: [Item] = [
    // First, because it is the one that makes something out of nothing. Everything below
    // needs a pane to already be there.
    Item(
      title: "New Tab", action: #selector(MusterWindow.newTab(_:)), key: "t",
      modifiers: [.command]),
    Item(
      title: "Split Right", action: #selector(MusterWindow.splitRight(_:)), key: "d",
      modifiers: [.command]),
    Item(
      title: "Split Down", action: #selector(MusterWindow.splitDown(_:)), key: "D",
      modifiers: [.command, .shift]),
    Item(
      title: "Close Pane", action: #selector(MusterWindow.closePane(_:)), key: "w",
      modifiers: [.command]),
    Item(
      title: "Next Pane", action: #selector(MusterWindow.focusNextPane(_:)), key: "]",
      modifiers: [.command]),
    Item(
      title: "Previous Pane", action: #selector(MusterWindow.focusPreviousPane(_:)), key: "[",
      modifiers: [.command]),
    // The movement a terminal user expects, and the one every other multiplexer has. Next and
    // previous already reach every pane, so these are ergonomics rather than reachability -
    // which is why they are allowed to go nowhere at an edge instead of wrapping.
    //
    // Arrow keys as key equivalents, spelled by the unicode codepoints AppKit wants for them.
    Item(
      title: "Select Pane Left", action: #selector(MusterWindow.focusPaneLeft(_:)),
      key: "\u{F702}", modifiers: [.command, .option]),
    Item(
      title: "Select Pane Right", action: #selector(MusterWindow.focusPaneRight(_:)),
      key: "\u{F703}", modifiers: [.command, .option]),
    Item(
      title: "Select Pane Above", action: #selector(MusterWindow.focusPaneUp(_:)),
      key: "\u{F700}", modifiers: [.command, .option]),
    Item(
      title: "Select Pane Below", action: #selector(MusterWindow.focusPaneDown(_:)),
      key: "\u{F701}", modifiers: [.command, .option]),
  ]

  /// Builds the smallest menu bar that makes the platform's shortcuts work.
  ///
  /// An app with no menu at all is one a person cannot quit normally, and one whose ⌘V does
  /// nothing.
  ///
  /// Copy is deliberately absent - it needs a selection, and a pane's selection lives in the
  /// daemon where Muster cannot yet reach it. A menu item that silently does nothing would be
  /// worse than its absence.
  public static func build(target: AnyObject) -> NSMenu {
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
    editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
    editItem.submenu = editMenu
    menu.addItem(editItem)

    let paneItem = NSMenuItem()
    let paneMenu = NSMenu(title: "Pane")
    for item in paneItems {
      // An explicit target rather than the responder chain, because the first responder is a
      // surface and these are not a surface's business. A chain walk would also make ⌘W mean
      // "close the window" the moment no pane has focus, which is not what it says.
      let entry = NSMenuItem(title: item.title, action: item.action, keyEquivalent: item.key)
      entry.keyEquivalentModifierMask = item.modifiers
      entry.target = target
      paneMenu.addItem(entry)
    }
    paneItem.submenu = paneMenu
    menu.addItem(paneItem)

    return menu
  }
}
