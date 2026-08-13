/// A keystroke, in Muster's own terms.
///
/// OS-free on purpose. The shell fills this in from whatever its platform hands it, and
/// everything downstream - the keymap, the encoder, the tests - reads only this. That is
/// what lets key handling be decided in a headless core and exercised as a table
/// (`docs/testing.md`), rather than needing a window and a keyboard.
///
/// The fields are the ones a terminal encoder needs, and no others. `text` and
/// `unshiftedCodepoint` both exist because they answer different questions: what the
/// layout produced, and what the key is called when modifiers are ignored.
public struct KeyEvent: Equatable, Sendable {
  public enum Action: Sendable {
    case press
    case release
    /// Auto-repeat. Distinct from a press because the kitty protocol reports it
    /// separately, and a program that tracks held keys needs the difference.
    case repeated
  }

  public let action: Action
  public let key: Key
  public let modifiers: Modifiers

  /// The modifiers the keyboard layout already spent producing `text`.
  ///
  /// Encoders must not report these a second time: on a German layout, option+8
  /// produces `{`, and an encoder told both "option is down" and "the text is {" would
  /// send an escape sequence for alt+{ instead of the brace the user typed.
  public let consumedModifiers: Modifiers

  /// What the layout produced, if anything. Empty for keys with no text, such as arrows.
  public let text: String

  /// The codepoint this key produces with no modifiers at all.
  ///
  /// The kitty protocol reports it so an application can recognize a chord by the key
  /// the user sees printed on the cap, independent of shift state and layout.
  public let unshiftedCodepoint: Unicode.Scalar?

  /// Whether an input method is mid-composition.
  ///
  /// A composing keystroke belongs to the input method, not to the pane: it must not be
  /// encoded and sent, or typing Japanese would deliver the romaji as well as the
  /// result.
  public let isComposing: Bool

  public init(
    action: Action = .press,
    key: Key,
    modifiers: Modifiers = [],
    consumedModifiers: Modifiers = [],
    text: String = "",
    unshiftedCodepoint: Unicode.Scalar? = nil,
    isComposing: Bool = false
  ) {
    self.action = action
    self.key = key
    self.modifiers = modifiers
    self.consumedModifiers = consumedModifiers
    self.text = text
    self.unshiftedCodepoint = unshiftedCodepoint
    self.isComposing = isComposing
  }
}

/// Which modifier keys were down.
///
/// The bit positions match libghostty-vt's `GhosttyMods` so the encoder seam is a cast
/// rather than a translation. That is a deliberate coupling to a published ABI, and
/// `MusterVTTests` pins it: if a pin bump renumbers these, a test fails rather than
/// every chord quietly encoding as something else.
public struct Modifiers: OptionSet, Hashable, Sendable {
  public let rawValue: UInt16

  public init(rawValue: UInt16) {
    self.rawValue = rawValue
  }

  public static let shift = Modifiers(rawValue: 1 << 0)
  public static let control = Modifiers(rawValue: 1 << 1)
  /// Alt on a PC keyboard, option on a Mac.
  public static let alt = Modifiers(rawValue: 1 << 2)
  /// Command on a Mac, Windows key elsewhere.
  public static let `super` = Modifiers(rawValue: 1 << 3)
  public static let capsLock = Modifiers(rawValue: 1 << 4)
  public static let numLock = Modifiers(rawValue: 1 << 5)

  /// Side bits. Only meaningful when the matching modifier bit is set, and only on
  /// platforms that report the difference.
  public static let shiftIsRight = Modifiers(rawValue: 1 << 6)
  public static let controlIsRight = Modifiers(rawValue: 1 << 7)
  public static let altIsRight = Modifiers(rawValue: 1 << 8)
  public static let superIsRight = Modifiers(rawValue: 1 << 9)
}

extension Modifiers {
  /// The wire names for each bit, in bit order.
  ///
  /// Muster's vocabulary has to survive leaving this process - into the conformance
  /// corpus, into the log, and into the schema the shell and core speak (MIP-1). A
  /// bitmask does none of that legibly: `520` in a case file tells a reader nothing, and
  /// silently means something different if the bits are ever renumbered.
  public static let allNames: [(name: String, bit: Modifiers)] = [
    ("shift", .shift),
    ("control", .control),
    ("alt", .alt),
    ("super", .`super`),
    ("capsLock", .capsLock),
    ("numLock", .numLock),
    ("shiftIsRight", .shiftIsRight),
    ("controlIsRight", .controlIsRight),
    ("altIsRight", .altIsRight),
    ("superIsRight", .superIsRight),
  ]

  /// The names of the bits that are set, in bit order so two runs agree.
  public var names: [String] {
    Self.allNames.filter { contains($0.bit) }.map(\.name)
  }

  /// Reads a set of names, or nil if any is not a modifier.
  ///
  /// Fails rather than ignoring the unknown one: a case file that says `"comand"` should
  /// say so, not quietly test an unmodified key and pass.
  public init?(names: [String]) {
    var result = Modifiers()
    for name in names {
      guard let match = Self.allNames.first(where: { $0.name == name }) else { return nil }
      result.insert(match.bit)
    }
    self = result
  }
}
