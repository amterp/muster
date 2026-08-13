/// The pane terminal modes an encoder needs, and Muster has to guess.
///
/// This type exists because of a gap, and it should be read as one. Encoding a keystroke
/// correctly requires knowing what the program in the pane asked for - the kitty
/// keyboard flags it pushed, whether it put the cursor keys in application mode, whether
/// it turned on bracketed paste. That state lives in the daemon's terminal. herdr
/// exposes none of it, and the frame stream consumes the sequences that set it, so
/// Muster cannot read it or observe it (`docs/observations/herdr-0.8.0.md` sections 2
/// and 5).
///
/// So every keystroke Muster sends is encoded against an assumption. Keeping that
/// assumption in one named type means it is a seam to feed from truth the day herdr
/// publishes its `InputState`, rather than a set of literals spread across the input
/// path that nobody can find again.
public struct TerminalModeProfile: Equatable, Sendable {
  /// Kitty keyboard protocol flags, as a bitmask.
  ///
  /// See `KittyFlags`. Zero means the legacy encoding.
  public let kittyFlags: UInt8

  /// DECCKM. Decides `SS3 A` against `CSI A` for the arrow keys.
  public let applicationCursorKeys: Bool

  /// DECKPAM. Decides whether the numeric keypad sends application sequences.
  public let applicationKeypad: Bool

  /// Whether alt-modified keys are sent as an ESC prefix rather than as a high bit.
  public let altSendsEscapePrefix: Bool

  /// xterm's modifyOtherKeys level 2, an older answer to the same problem kitty solves.
  public let modifyOtherKeys: Bool

  /// DEC 2004. Decides whether pasted text is wrapped in paste markers.
  ///
  /// The one mode where a wrong guess is visibly destructive rather than merely lossy:
  /// markers sent to a program that never enabled the mode arrive as literal text, which
  /// is exactly what a recorded probe caught herdr passing through
  /// (`corpus/herdr-0.8.0/input-encoding/`).
  public let bracketedPaste: Bool

  /// Whether the macOS option key acts as alt rather than composing text.
  ///
  /// Not a pane mode - a local preference, and the one field here that is not a guess.
  ///
  /// Four-valued rather than a flag because the per-side settings are the ones people
  /// actually pick: option composes accented characters on macOS, so a common
  /// arrangement is right-option-as-alt for meta chords with left option still
  /// composing.
  public let optionActsAsAlt: OptionAsAlt

  public enum OptionAsAlt: Sendable {
    case never
    case always
    case leftOnly
    case rightOnly
  }

  public init(
    kittyFlags: UInt8 = 0,
    applicationCursorKeys: Bool = false,
    applicationKeypad: Bool = false,
    altSendsEscapePrefix: Bool = true,
    modifyOtherKeys: Bool = false,
    bracketedPaste: Bool = false,
    optionActsAsAlt: OptionAsAlt = .never
  ) {
    self.kittyFlags = kittyFlags
    self.applicationCursorKeys = applicationCursorKeys
    self.applicationKeypad = applicationKeypad
    self.altSendsEscapePrefix = altSendsEscapePrefix
    self.modifyOtherKeys = modifyOtherKeys
    self.bracketedPaste = bracketedPaste
    self.optionActsAsAlt = optionActsAsAlt
  }

  /// What Muster assumes about a pane it knows nothing about.
  ///
  /// A terminal in its power-on state: what every mode is before a program changes it.
  /// That makes the guess a bounded one rather than an invented one - it is the same
  /// state herdr's own TUI presents to a pane whose program has negotiated nothing, and
  /// the bytes it produces are the bytes a ghostty user's programs already receive.
  ///
  /// The choice is deliberately asymmetric, because the two ways of being wrong do not
  /// cost the same. Guessing *low* degrades: a program that negotiated the kitty
  /// protocol still understands these sequences, since the protocol is additive.
  /// Guessing *high* breaks: a shell that never enabled kitty receives `\u{1b}[97u` and
  /// prints it, because herdr writes our bytes to the pane's PTY untouched.
  ///
  /// What this actually costs is narrower than it first looks. Shift+enter - the
  /// distinction an agent needs most - survives, because ghostty encodes it as
  /// `CSI 27;2;13~` in every mode rather than only under kitty
  /// (`src/input/function_keys.zig:199`, no mode qualifier on the entry). The real
  /// losses are `applicationCursorKeys`, which decides whether arrow keys work in vim
  /// and less, and `bracketedPaste`, which decides whether a paste arrives as text or as
  /// text wrapped in visible markers.
  ///
  /// Replace it with truth, not with a better guess. Both of those losses also have a
  /// fallback that needs no upstream change: `pane.send_input` encodes text and named
  /// keys server-side against the pane's real modes, at the cost of a socket connect per
  /// call. Whether that trade is worth making waits on the latency measurement.
  public static let unknownPane = TerminalModeProfile()

  /// What herdr's own TUI negotiates with its host terminal.
  ///
  /// Three kitty flags, not the full set: `ime_compatible_keyboard_enhancement_flags`
  /// (herdr `src/input/model.rs:219`) is named for the reason it stops there, and
  /// `:427` asserts that reporting all keys as escape codes stays out because it breaks
  /// IME. Reachable once the modes can be read; kept here so the target is written down.
  public static let herdrTUI = TerminalModeProfile(
    kittyFlags: KittyFlags.disambiguate | KittyFlags.reportEventTypes
      | KittyFlags.reportAlternateKeys)

  /// Kitty keyboard protocol flag bits.
  public enum KittyFlags {
    public static let disambiguate: UInt8 = 1
    public static let reportEventTypes: UInt8 = 2
    public static let reportAlternateKeys: UInt8 = 4
    public static let reportAllKeysAsEscapeCodes: UInt8 = 8
    public static let reportAssociatedText: UInt8 = 16
  }
}
