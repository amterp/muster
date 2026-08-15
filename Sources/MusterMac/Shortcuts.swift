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

  /// How the list is drawn, in one place because the sizing depends on it.
  ///
  /// A window like this should open at the size that fits it, so nothing truncates and there
  /// is nothing to scroll - which means the fonts and the row heights are not decoration but
  /// inputs to how big the window is. Anything else Muster grows of this kind (a settings
  /// window, another help surface) should size itself the same way.
  @MainActor
  public enum Metrics {
    public static let title = NSFont.systemFont(ofSize: 13)
    /// The system font rather than a monospaced one. These are modifier glyphs, which the
    /// system font draws far more legibly at this size, and the column lines up because it
    /// is right-aligned rather than because the characters are all one width - which is how
    /// the platform's own menus do it.
    public static let chord = NSFont.systemFont(ofSize: 13)
    public static let header = NSFont.systemFont(ofSize: 11, weight: .semibold)

    public static let rowHeight: CGFloat = 26
    /// Taller, because the space above a heading is what separates one group from the last.
    public static let headerHeight: CGFloat = 34
    public static let inset: CGFloat = 16
    /// The least space between what something is called and the chord that does it, so the
    /// two columns never read as one.
    public static let gap: CGFloat = 32
  }

  /// How wide each column has to be for nothing in it to be cut off.
  ///
  /// Measured from the text rather than guessed, because the longest row decides: "Go to a
  /// pane nothing is showing" is the reason a fixed width truncated.
  public static func columnWidths(_ sections: [Section]) -> (title: CGFloat, detail: CGFloat) {
    var title: CGFloat = 0
    var detail: CGFloat = 0
    for section in sections {
      for row in section.rows {
        title = max(title, width(row.title, Metrics.title))
        let trailing = row.chord.isEmpty ? row.note : row.chord
        let font = row.chord.isEmpty ? Metrics.title : Metrics.chord
        detail = max(detail, width(trailing, font))
      }
    }
    return (title.rounded(.up), detail.rounded(.up))
  }

  /// The size the window wants, and the size it is allowed.
  ///
  /// Clamped to what it is given rather than assumed to fit: a laptop screen in a meeting
  /// room is smaller than the list, and a window taller than the display is worse than a
  /// scroll bar. The scroller stays for that case and hides itself the rest of the time.
  public static func windowSize(_ sections: [Section], limit: CGSize) -> CGSize {
    let columns = columnWidths(sections)
    let width = Metrics.inset * 2 + columns.title + Metrics.gap + columns.detail
    let height = sections.reduce(CGFloat.zero) { running, section in
      running + Metrics.headerHeight + CGFloat(section.rows.count) * Metrics.rowHeight
    }
    return CGSize(
      width: min(max(width, 320), limit.width),
      height: min(height + Metrics.inset, limit.height))
  }

  private static func width(_ text: String, _ font: NSFont) -> CGFloat {
    (text as NSString).size(withAttributes: [.font: font]).width
  }

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
