/// What Muster does with a keystroke before the pane sees it.
///
/// Input precedence is fixed (architecture.md): the keymap gets first refusal on every
/// chord, and only what it declines is reported toward the focused pane. That ordering
/// is structural here from the first keystroke rather than retrofitted later, because
/// retrofitting it means finding every place that already sends bytes.
///
/// The bindings here are defaults, not configuration. A config file replaces the table
/// without changing the path it is consulted on.
public struct Keymap: Sendable {
  private let bindings: [Binding: Resolution]

  public init(bindings: [Binding: Resolution] = Keymap.macOSTextEditing) {
    self.bindings = bindings
  }

  /// A chord: which key, under which modifiers.
  public struct Binding: Hashable, Sendable {
    public let key: Key
    public let modifiers: Modifiers

    public init(_ key: Key, _ modifiers: Modifiers) {
      self.key = key
      self.modifiers = modifiers
    }
  }

  /// What a chord resolves to.
  public enum Resolution: Equatable, Sendable {
    /// Muster handles it; the pane never sees it.
    case action(Action)
    /// Muster substitutes these bytes for whatever the encoder would have produced.
    case text([UInt8])
    /// Not bound. Report it to the pane.
    case unbound
  }

  /// The operations a chord can be bound to.
  ///
  /// Empty until there is something to bind. Kept as a type rather than a comment so the
  /// dispatcher has somewhere to grow, and so `Resolution` is not a lie about a shape
  /// that does not exist.
  public enum Action: Equatable, Sendable {}

  public func resolve(_ key: KeyEvent) -> Resolution {
    guard key.action != .release else { return .unbound }
    // Only the modifiers that pick a binding. A chord is the same chord whichever side of
    // the keyboard supplied its command key.
    let held = key.modifiers.intersection([.shift, .control, .alt, .super])
    return bindings[Binding(key.key, held)] ?? .unbound
  }

  /// The line-editing chords macOS users expect a terminal to honor.
  ///
  /// These are not terminal conventions - no program asks for them and no mode enables
  /// them. They are the text-editing shortcuts every other macOS app has, which people
  /// reasonably keep pressing in a terminal, and each one maps onto the readline control
  /// code that does the same job. Without them ⌘⌫ deletes a single character and looks
  /// broken.
  ///
  /// Taken from ghostty, whose macOS build binds exactly these five and calls them
  /// "natural text editing" (`src/config/Config.zig`, in the macOS keybind defaults).
  /// Matching it is the point rather than a coincidence: Muster promises the platform's
  /// own keybindings, and a person moving between the two terminals should not have to
  /// learn which is which.
  ///
  /// They also sidestep the mode problem, since a control code means the same thing
  /// whatever the pane has negotiated.
  public static let macOSTextEditing: [Binding: Resolution] = [
    // Start and end of line, which readline spells ctrl+A and ctrl+E.
    Binding(.arrowLeft, .super): .text([0x01]),
    Binding(.arrowRight, .super): .text([0x05]),
    // Delete to start of line: readline's unix-line-discard.
    Binding(.backspace, .super): .text([0x15]),
    // Word motion, as an escape prefix rather than a control code.
    Binding(.arrowLeft, .alt): .text([0x1b, UInt8(ascii: "b")]),
    Binding(.arrowRight, .alt): .text([0x1b, UInt8(ascii: "f")]),
  ]
}
