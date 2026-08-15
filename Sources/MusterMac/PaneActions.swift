import AppKit

/// What each of Muster's actions is called on screen, and what carries it out here.
///
/// The shell's half of the vocabulary the core owns. The core says which chord asks for
/// `split_right`; this says that `split_right` is called "Split Right" and is done by
/// `MusterWindow.splitRight`. Both halves are needed to build a menu and neither belongs to
/// the other: a title is UI text, and a selector is this platform's way of dispatching.
///
/// An action the core names and this does not know is skipped rather than guessed at, so a
/// core one version ahead produces a menu missing an item rather than a crash.
@MainActor
public enum PaneActions {
  public struct Described {
    public let title: String
    public let selector: Selector
  }

  /// Keyed by the core's own name for the action.
  public static let byName: [String: Described] = [
    "new_tab": Described(title: "New Tab", selector: #selector(MusterWindow.newTab(_:))),
    "split_right": Described(
      title: "Split Right", selector: #selector(MusterWindow.splitRight(_:))),
    "split_down": Described(title: "Split Down", selector: #selector(MusterWindow.splitDown(_:))),
    "close_pane": Described(title: "Close Pane", selector: #selector(MusterWindow.closePane(_:))),
    "next_pane": Described(
      title: "Next Pane", selector: #selector(MusterWindow.focusNextPane(_:))),
    "previous_pane": Described(
      title: "Previous Pane", selector: #selector(MusterWindow.focusPreviousPane(_:))),
    "focus_left": Described(
      title: "Select Pane Left", selector: #selector(MusterWindow.focusPaneLeft(_:))),
    "focus_right": Described(
      title: "Select Pane Right", selector: #selector(MusterWindow.focusPaneRight(_:))),
    "focus_up": Described(
      title: "Select Pane Above", selector: #selector(MusterWindow.focusPaneUp(_:))),
    "focus_down": Described(
      title: "Select Pane Below", selector: #selector(MusterWindow.focusPaneDown(_:))),
    "resize_left": Described(
      title: "Resize Pane Left", selector: #selector(MusterWindow.resizePaneLeft(_:))),
    "resize_right": Described(
      title: "Resize Pane Right", selector: #selector(MusterWindow.resizePaneRight(_:))),
    "resize_up": Described(
      title: "Resize Pane Up", selector: #selector(MusterWindow.resizePaneUp(_:))),
    "resize_down": Described(
      title: "Resize Pane Down", selector: #selector(MusterWindow.resizePaneDown(_:))),
    "zoom": Described(title: "Zoom Pane", selector: #selector(MusterWindow.zoomPane(_:))),
    "toggle_sidebar": Described(
      title: "Show Agents", selector: #selector(MusterWindow.toggleSidebar(_:))),
  ]
}

/// One key, in the character AppKit wants as a menu item's key equivalent.
///
/// AppKit takes a character rather than a key code, so the W3C name the core speaks has to be
/// turned back into one. Lowercase throughout, with shift carried in the modifier mask: a
/// capital in this field is AppKit's own way of spelling shift, and setting both is how an
/// item ends up needing shift pressed twice.
///
/// `nil` for a key with no character - a shell cannot put those on a menu, and an item with an
/// empty key equivalent is one that silently has no shortcut at all.
public func menuKeyEquivalent(forKeyNamed name: String) -> String? {
  if name.hasPrefix("Key"), name.count == 4 {
    return String(name.dropFirst(3)).lowercased()
  }
  if name.hasPrefix("Digit"), name.count == 6 {
    return String(name.dropFirst(5))
  }
  if name.hasPrefix("F"), let number = Int(name.dropFirst()), (1...15).contains(number) {
    // The function keys, at the codepoints AppKit reserves for them.
    return String(UnicodeScalar(0xF704 + number - 1)!)
  }
  switch name {
  case "ArrowUp": return "\u{F700}"
  case "ArrowDown": return "\u{F701}"
  case "ArrowLeft": return "\u{F702}"
  case "ArrowRight": return "\u{F703}"
  case "Enter": return "\r"
  case "Tab": return "\t"
  case "Space": return " "
  case "Escape": return "\u{1B}"
  case "Backspace": return "\u{8}"
  case "Delete": return "\u{7F}"
  case "Home": return "\u{F729}"
  case "End": return "\u{F72B}"
  case "PageUp": return "\u{F72C}"
  case "PageDown": return "\u{F72D}"
  case "BracketLeft": return "["
  case "BracketRight": return "]"
  case "Comma": return ","
  case "Period": return "."
  case "Slash": return "/"
  case "Semicolon": return ";"
  case "Quote": return "'"
  case "Minus": return "-"
  case "Equal": return "="
  case "Backslash": return "\\"
  case "Backquote": return "`"
  default: return nil
  }
}

/// The modifier names the core speaks, as AppKit's mask.
///
/// Anything unrecognized is ignored rather than refused: the core's list is the four a chord
/// can carry, and a fifth arriving means a core ahead of this shell rather than a menu worth
/// refusing to build.
public func menuModifiers(_ names: [String]) -> NSEvent.ModifierFlags {
  var flags: NSEvent.ModifierFlags = []
  for name in names {
    switch name {
    case "shift": flags.insert(.shift)
    case "control": flags.insert(.control)
    case "alt": flags.insert(.option)
    case "super": flags.insert(.command)
    default: break
    }
  }
  return flags
}
