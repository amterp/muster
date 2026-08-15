import AppKit

/// Everything Muster does and which key does it, in one list.
///
/// Discoverability was a documentation problem: a new window gives you a pane and no hint
/// that ⌘D splits it or that a list of agents is one chord away. Rebinding made that sharper
/// rather than softer - once a file can move any action, the chord somebody remembers and the
/// chord that is live can differ, and nothing in the app said which.
///
/// Two sources, and they are different kinds of thing. What the core publishes is right by
/// construction: it is the same table the menu is built from, so a rebind moves this list too,
/// and an action this shell had to skip is still listed. What the core does not publish is
/// everything else a person does here - the platform's own chords, and the things with no
/// chord at all - and a list covering only the first quietly implies the rest does not exist.
///
/// Read-only. Editing stays the config file, because a help surface that also rebinds is a
/// second way to do something the file already does, and the file is the one an agent and a
/// dotfiles repo can both read.
@MainActor
public enum Shortcuts {
  /// One line in the list.
  public struct Row: Equatable {
    public let title: String

    /// The chord, spelled the way a Mac shows one, or empty when there is none.
    ///
    /// Empty covers two different things on purpose: an action somebody unbound, and an
    /// interaction that never had a chord. Both are worth listing - the first because
    /// unbinding ⌘W was about the shortcut and not about the action, and the second because
    /// clicking a pane to focus it is a thing you can do and nothing else here says so.
    public let chord: String

    /// What it is for, when the title does not carry it. Empty for the obvious ones.
    public let note: String
  }

  public struct Section: Equatable {
    public let title: String
    public let rows: [Row]
  }

  /// The list, as the core has it bound right now.
  ///
  /// Grouped the way the menus are, then a section for what no menu holds. A person looking
  /// for a chord is looking for a thing they want to do, so the grouping is by what it does.
  public static func sections(_ bindings: [Core.Binding]) -> [Section] {
    var byGroup: [MenuActions.Group: [Row]] = [:]
    var unknown: [Row] = []
    for binding in bindings {
      guard let described = MenuActions.byName[binding.action] else {
        // An action the core names and this shell does not. Listed under its own name rather
        // than dropped: somebody pressing the chord and finding it works deserves to be able
        // to look it up, and a missing line is the one symptom nobody can search for.
        unknown.append(Row(title: binding.action, chord: spell(binding), note: ""))
        continue
      }
      byGroup[described.group, default: []].append(
        Row(title: described.title, chord: spell(binding), note: ""))
    }

    var sections = MenuActions.Group.allCases.compactMap { group -> Section? in
      guard let rows = byGroup[group], !rows.isEmpty else { return nil }
      return Section(title: group.rawValue, rows: rows)
    }
    if !unknown.isEmpty {
      sections.append(Section(title: "Newer than this window", rows: unknown))
    }
    sections.append(Section(title: "Editing", rows: editing))
    sections.append(Section(title: "With the mouse", rows: pointing))
    return sections
  }

  /// The chords this shell owns rather than the core.
  ///
  /// Declared rather than read back, because these are the platform's: copy and paste go
  /// through the responder chain and quit is the application's, so none of them is in the
  /// core's table and none of them can be rebound by Muster's config file. Listed anyway,
  /// since a person looking for "how do I copy" does not care which layer answers.
  static let editing: [Row] = [
    Row(title: "Copy", chord: "⌘C", note: "the selection in the focused pane"),
    Row(title: "Paste", chord: "⌘V", note: ""),
    Row(title: "Quit muster", chord: "⌘Q", note: "sessions keep running; agents are unaffected"),
  ]

  /// What you can do here that has no chord at all.
  ///
  /// The half a list built only from bindings would leave out, and the half a new window most
  /// needs: nothing on screen says that the divider between two panes can be dragged.
  static let pointing: [Row] = [
    Row(title: "Focus a pane", chord: "", note: "click it"),
    Row(title: "Go to a pane nothing is showing", chord: "", note: "click its row in the list"),
    Row(title: "Resize panes", chord: "", note: "drag the divider between them"),
    Row(title: "Scroll back", chord: "", note: "the wheel, over the pane"),
  ]

  /// One chord, spelled the way a Mac menu spells one.
  ///
  /// Modifiers in the platform's own order - control, option, shift, command - because that
  /// is the order they are printed in everywhere else on this machine, and a list that spelled
  /// them differently would be one more thing to translate.
  static func spell(_ binding: Core.Binding) -> String {
    if binding.key.isEmpty { return "" }
    var spelled = ""
    if binding.modifiers.contains("control") { spelled += "⌃" }
    if binding.modifiers.contains("alt") { spelled += "⌥" }
    if binding.modifiers.contains("shift") { spelled += "⇧" }
    if binding.modifiers.contains("super") { spelled += "⌘" }
    return spelled + printed(key: binding.key)
  }

  /// A key as it is printed on a menu, or on the cap when the menu has no glyph for it.
  static func printed(key: String) -> String {
    switch key {
    case "ArrowLeft": return "←"
    case "ArrowRight": return "→"
    case "ArrowUp": return "↑"
    case "ArrowDown": return "↓"
    case "Enter": return "↩"
    case "Tab": return "⇥"
    case "Space": return "Space"
    case "Escape": return "⎋"
    case "Backspace": return "⌫"
    case "Delete": return "⌦"
    // The punctuation keys, which the core names and a keyboard prints. Without these a row
    // reads `⌘Slash`, which is the wire's spelling showing through to somebody who wanted to
    // know which key to press.
    case "BracketLeft": return "["
    case "BracketRight": return "]"
    case "Slash": return "/"
    case "Backslash": return "\\"
    case "Comma": return ","
    case "Period": return "."
    case "Semicolon": return ";"
    case "Quote": return "'"
    case "Minus": return "-"
    case "Equal": return "="
    case "Backquote": return "`"
    default:
      // The friendly spelling the config file uses, which for a letter or digit is the
      // character itself. Anything else is left as the core named it rather than blanked:
      // a name somebody can search for beats a gap.
      if key.hasPrefix("Key"), key.count == 4 { return String(key.dropFirst(3)) }
      if key.hasPrefix("Digit"), key.count == 6 { return String(key.dropFirst(5)) }
      return key
    }
  }
}
