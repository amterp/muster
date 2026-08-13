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

  public init(bindings: [Binding: Resolution] = Keymap.defaults) {
    self.bindings = bindings
  }

  /// What Muster binds out of the box.
  ///
  /// Local bindings win a collision: a chord someone chose to bind should not lose to a
  /// key that merely wants encoding help.
  public static let defaults = macOSTextEditing.merging(modeSensitiveKeys) { local, _ in local }

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
    /// The backend encodes this one, under this name.
    ///
    /// For the keys where encoding locally is known to be wrong. Muster guesses the pane's
    /// terminal modes and the daemon does not have to.
    case serverEncoded(String)
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

  /// The keys whose correct encoding depends on a mode Muster cannot see.
  ///
  /// The arrows, and only the arrows, for a measured reason. Application cursor mode
  /// decides between `\u{1b}OA` and `\u{1b}[A`, and a program that trusts terminfo accepts
  /// only the first: `less` calls `smkx` on startup and then rings the bell at anything
  /// else. `vim` accepts both, which is why one program is not a survey. Nothing else in
  /// the guess was measured to break - shift+enter, dead keys and control chords all
  /// survive - so nothing else is routed the slow way.
  ///
  /// Unmodified only. herdr's key vocabulary does accept chords like `shift+up`, but a
  /// modified arrow is not what a pager reads, and every routed key costs a round trip.
  public static let modeSensitiveKeys: [Binding: Resolution] = [
    Binding(.arrowUp, []): .serverEncoded("up"),
    Binding(.arrowDown, []): .serverEncoded("down"),
    Binding(.arrowLeft, []): .serverEncoded("left"),
    Binding(.arrowRight, []): .serverEncoded("right"),
  ]
}
