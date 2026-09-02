/// What Muster should look like, in Muster's own words.
///
/// The renderer seam's half of the appearance vocabulary. Nothing here is libghostty-shaped:
/// these are the names the config file uses and the core publishes, and turning them into
/// something a renderer understands is `ghosttyConfiguration` below - the one place in Muster
/// that knows a ghostty config key exists.
///
/// Every value is optional, and absent means the renderer's own default. Muster names none of
/// its own: what this vocabulary is for is saying what a person changed.
public struct Appearance: Equatable, Sendable {
  /// A family name as the system knows it. Absent leaves the choice to the renderer, which is
  /// a question about the machine rather than about Muster.
  public var fontFamily: String?
  /// In points.
  public var fontSize: Float?

  // Colours as the core spells them: `#rrggbb`, already refused if they were anything else.
  // Carried as strings rather than parsed into a colour type because nothing here looks at
  // them - the shell hands them straight on, and a second parser would be a second thing to
  // disagree with the first.
  public var background: String?
  public var foreground: String?
  public var cursor: String?
  public var cursorText: String?
  public var selectionBackground: String?
  public var selectionForeground: String?

  /// What bold text is painted in. Absent leaves bold whatever colour the text already had,
  /// which is what a terminal does - and why an agent's output, which is full of `**bold**`,
  /// reads flat without this.
  public var bold: String?

  /// The sixteen ANSI colours, or none of them. Never a partial list - the core refuses one.
  public var palette: [String]

  public var cursorStyle: CursorStyle?
  /// Absent leaves it to the program in the pane, which can ask for either and often does.
  public var cursorBlink: Bool?

  /// Blank space between a pane's text and its edges, in points. Zero is a real answer.
  public var panePadding: UInt32?

  public init(
    fontFamily: String? = nil, fontSize: Float? = nil,
    background: String? = nil, foreground: String? = nil,
    cursor: String? = nil, cursorText: String? = nil,
    selectionBackground: String? = nil, selectionForeground: String? = nil,
    bold: String? = nil,
    palette: [String] = [],
    cursorStyle: CursorStyle? = nil, cursorBlink: Bool? = nil,
    panePadding: UInt32? = nil
  ) {
    self.fontFamily = fontFamily
    self.fontSize = fontSize
    self.background = background
    self.foreground = foreground
    self.cursor = cursor
    self.cursorText = cursorText
    self.selectionBackground = selectionBackground
    self.selectionForeground = selectionForeground
    self.bold = bold
    self.palette = palette
    self.cursorStyle = cursorStyle
    self.cursorBlink = cursorBlink
    self.panePadding = panePadding
  }

  /// The shapes a cursor comes in, in Muster's spelling.
  public enum CursorStyle: String, Sendable {
    case block
    case bar
    case underline
    /// An outline rather than a filled block.
    case hollow
  }
}

/// Muster's appearance as libghostty's own configuration file.
///
/// The whole of the translation, and the only function in Muster that knows what a ghostty
/// config key is called. A file rather than anything in memory because libghostty has no
/// setter: `ghostty_config_new` fills in defaults and the only ways to change one are the four
/// `load_*` functions (docs/observations/libghostty-9f9b8d1d.md section 9). Feeding it a
/// synthesized argv works too and needs nothing on disk, but only once per process - so a file
/// is what lets a reload use the same path as a launch instead of a second one that can
/// disagree with it.
///
/// Empty when the appearance names nothing, which is how a person who configured no appearance
/// gets no file rather than an empty one.
public func ghosttyConfiguration(_ appearance: Appearance) -> [String] {
  var lines: [String] = []
  func set(_ key: String, _ value: String?) {
    guard let value else { return }
    lines.append("\(key) = \(value)")
  }

  set("font-family", appearance.fontFamily)
  // Formatted without a trailing `.0`, because a size is a number somebody wrote as `13` and
  // seeing `13.0` come back in a file named after another program is one more thing to wonder
  // about while debugging.
  set("font-size", appearance.fontSize.map { $0 == $0.rounded() ? String(Int($0)) : String($0) })

  set("background", appearance.background)
  set("foreground", appearance.foreground)
  set("cursor-color", appearance.cursor)
  set("cursor-text", appearance.cursorText)
  set("selection-background", appearance.selectionBackground)
  set("selection-foreground", appearance.selectionForeground)
  // ghostty spells this one `bold-color` and also takes `bright` there, which Muster does not
  // offer: every other key in `[colors]` is a colour, and a second spelling meaning "the bright
  // slot of whatever hue this already was" is a different setting wearing the same name.
  set("bold-color", appearance.bold)

  // One line per colour, which is how a repeatable key accumulates. Indexed from zero, matching
  // the order the core publishes them in: black through bright white.
  for (index, color) in appearance.palette.enumerated() {
    lines.append("palette = \(index)=\(color)")
  }

  // `hollow` is Muster's word and `block_hollow` is ghostty's. The translation lives here
  // precisely so that the difference never reaches the config file a person writes.
  set(
    "cursor-style",
    appearance.cursorStyle.map { $0 == .hollow ? "block_hollow" : $0.rawValue })
  set("cursor-style-blink", appearance.cursorBlink.map(String.init))

  // One number for both axes, matching `resize_step`: which side of a pane the space is on is
  // not a distinction anybody has asked for, and two keys would be two things to keep in step.
  if let padding = appearance.panePadding {
    lines.append("window-padding-x = \(padding)")
    lines.append("window-padding-y = \(padding)")
  }

  return lines
}
