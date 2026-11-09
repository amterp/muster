import AppKit
import MusterRenderer

/// A color as three sRGB components from 0 to 1.
///
/// The palette below is arithmetic on colors - mixing them and measuring contrast between them -
/// and that wants plain numbers rather than `NSColor`, whose dynamic colors have no components
/// until somebody says which appearance to resolve them in.
public struct RGB: Equatable, Sendable {
  public var red: Double
  public var green: Double
  public var blue: Double

  public init(red: Double, green: Double, blue: Double) {
    self.red = red
    self.green = green
    self.blue = blue
  }

  /// `#rrggbb`, as the core and libghostty spell a color. Nil for anything else.
  public init?(hex: String?) {
    guard let hex else { return nil }
    let digits = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
    guard digits.count == 6, let value = UInt32(digits, radix: 16) else { return nil }
    self.init(
      red: Double((value >> 16) & 0xff) / 255, green: Double((value >> 8) & 0xff) / 255,
      blue: Double(value & 0xff) / 255)
  }

  public static let black = RGB(red: 0, green: 0, blue: 0)
  public static let white = RGB(red: 1, green: 1, blue: 1)

  public var hex: String {
    let byte = { (component: Double) in Int((min(max(component, 0), 1) * 255).rounded()) }
    return String(format: "#%02x%02x%02x", byte(red), byte(green), byte(blue))
  }

  /// This color `amount` of the way toward `other`: 0 is this one, 1 is the other.
  public func mixed(toward other: RGB, _ amount: Double) -> RGB {
    RGB(
      red: red + (other.red - red) * amount, green: green + (other.green - green) * amount,
      blue: blue + (other.blue - blue) * amount)
  }

  /// WCAG's relative luminance.
  public var luminance: Double {
    let linear = { (component: Double) in
      component <= 0.04045 ? component / 12.92 : pow((component + 0.055) / 1.055, 2.4)
    }
    return 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
  }

  /// WCAG's contrast ratio between two colors, from 1 to 21.
  public func contrast(with other: RGB) -> Double {
    let (high, low) = (max(luminance, other.luminance), min(luminance, other.luminance))
    return (high + 0.05) / (low + 0.05)
  }

  /// The least contrast this color has with any of `surfaces`.
  func contrast(againstAll surfaces: [RGB]) -> Double {
    surfaces.map { contrast(with: $0) }.min() ?? 21
  }

  /// This color moved toward `target` until it reaches `minimum` contrast against every one of
  /// `surfaces`, or all the way to `target` if it never does.
  ///
  /// When `target` itself falls short - a terminal whose foreground is close to its background -
  /// the move goes toward whichever of black and white contrasts more instead, since text that
  /// cannot be read is worse than text that is not the terminal's color.
  func pushed(toward target: RGB, until minimum: Double, on surfaces: [RGB]) -> RGB {
    var goal = target
    if goal.contrast(againstAll: surfaces) < minimum {
      goal =
        RGB.black.contrast(againstAll: surfaces) >= RGB.white.contrast(againstAll: surfaces)
        ? .black : .white
    }
    var amount = 0.0
    var color = self
    while color.contrast(againstAll: surfaces) < minimum, amount < 1 {
      amount = min(amount + 0.02, 1)
      color = mixed(toward: goal, amount)
    }
    return color
  }
}

/// Every color the redesign's new surfaces are drawn with, derived from colors that already exist.
///
/// Muster has no theme of its own (Fencil decision `chrome-colors`): the agent list keeps the
/// platform's colors, a pane's chrome takes the terminal's background and foreground, and a
/// state's tint is that state's `agent_*` color mixed into the surface it sits on, so repainting
/// `agent_blocked` repaints the tint. Nothing here is a setting.
///
/// One pure function (`make`) so that a color that loses contrast on an unusual theme - a light
/// terminal beside a dark-mode list - is a test that fails rather than a window somebody squints
/// at. Text is pushed toward the foreground until it reaches `minimumContrast` against every
/// surface it can sit on.
public struct ChromePalette: Equatable, Sendable {
  /// WCAG's floor for body text.
  public static let minimumContrast = 4.5

  /// How far a tint is mixed from its surface toward the state's color.
  static let tintAmount = 0.14
  /// How far a raised panel and a header sit from the surface they stand on, toward its text.
  static let raisedAmount = 0.06
  static let keycapFillAmount = 0.08
  static let keycapEdgeAmount = 0.26

  /// The terminal's colors, as the config file or libghostty's default has them.
  public struct Terminal: Equatable, Sendable {
    public var background: RGB
    public var foreground: RGB

    public init(background: RGB, foreground: RGB) {
      self.background = background
      self.foreground = foreground
    }
  }

  /// The agent list's own colors, which are the platform's: its background and its text, resolved
  /// in whichever appearance the list is drawn in.
  public struct ListAppearance: Equatable, Sendable {
    public var background: RGB
    public var foreground: RGB
    /// The platform's accent color, which marks what is addressed to the person.
    public var accent: RGB

    public init(
      background: RGB, foreground: RGB, accent: RGB = RGB(red: 0.0, green: 0.48, blue: 1.0)
    ) {
      self.background = background
      self.foreground = foreground
      self.accent = accent
    }
  }

  /// The seven states an agent is painted in, by the backend's own spelling.
  public static let states = ["working", "blocked", "stuck", "waiting", "done", "idle", "unknown"]

  /// What is drawn on one background: the agent list's, or a pane header's.
  public struct Surface: Equatable, Sendable {
    public var background: RGB
    /// A panel standing a little above `background`: the tab on screen, in the list.
    public var raised: RGB
    public var text: RGB
    /// Secondary text, at the same contrast floor.
    public var dimText: RGB
    /// The background of something marked with a state: `background` with the state's color in it.
    public var tint: [String: RGB]
    /// A state's word or glyph as text, readable on `background`, `raised` and its own `tint`.
    public var stateText: [String: RGB]
    public var keycapFill: RGB
    public var keycapEdge: RGB
    public var keycapText: RGB
    /// The accent mixed into `background`: what a message to the person sits on.
    public var accentTint: RGB
    /// The accent as text or a bar, readable on `background`, `raised` and `accentTint`.
    public var accentText: RGB
  }

  public var list: Surface
  public var header: Surface
  /// The pane area: what the group conversation and the message banner are drawn on.
  public var pane: Surface

  /// Every derived color for these three inputs.
  ///
  /// `agents` is the seven states' colors as they are painted now, by state name; a state missing
  /// from it takes the `unknown` one, or gray.
  public static func make(
    terminal: Terminal, agents: [String: RGB], appearance: ListAppearance
  ) -> ChromePalette {
    ChromePalette(
      list: surface(
        background: appearance.background, text: appearance.foreground, agents: agents,
        accent: appearance.accent),
      header: surface(
        background: terminal.background.mixed(toward: terminal.foreground, raisedAmount),
        text: terminal.foreground, agents: agents, accent: appearance.accent),
      pane: surface(
        background: terminal.background, text: terminal.foreground, agents: agents,
        accent: appearance.accent))
  }

  private static func surface(
    background: RGB, text foreground: RGB, agents: [String: RGB], accent: RGB
  ) -> Surface {
    // A panel standing above the background, but never so far that the text cannot be read on it:
    // on a mid-gray background the lighter panel is the surface text has the least contrast with,
    // and no color can reach the floor on both.
    var raised = background.mixed(toward: foreground, raisedAmount)
    var lift = raisedAmount
    while foreground.contrast(againstAll: [raised]) < minimumContrast, lift > 0.004 {
      lift /= 2
      raised = background.mixed(toward: foreground, lift)
    }
    var tint: [String: RGB] = [:]
    var stateText: [String: RGB] = [:]
    for state in states {
      let color = agents[state] ?? agents["unknown"] ?? RGB(red: 0.5, green: 0.5, blue: 0.5)
      let tinted = background.mixed(toward: color, tintAmount)
      tint[state] = tinted
      stateText[state] = color.pushed(
        toward: foreground, until: minimumContrast, on: [background, raised, tinted])
    }
    // Body text also sits on the accent's tint, which is where a message to the person is drawn.
    let accentTint = background.mixed(toward: accent, tintAmount)
    let text = foreground.pushed(
      toward: foreground, until: minimumContrast, on: [background, raised, accentTint])
    let dim = text.mixed(toward: background, 0.3).pushed(
      toward: text, until: minimumContrast, on: [background, raised, accentTint])
    let keycapFill = background.mixed(toward: text, keycapFillAmount)
    return Surface(
      background: background, raised: raised, text: text, dimText: dim, tint: tint,
      stateText: stateText, keycapFill: keycapFill,
      keycapEdge: background.mixed(toward: text, keycapEdgeAmount),
      keycapText: text.pushed(toward: text, until: minimumContrast, on: [keycapFill]),
      accentTint: accentTint,
      accentText: accent.pushed(
        toward: foreground, until: minimumContrast, on: [background, raised, accentTint]))
  }
}

/// Where the palette's inputs come from in the running app.
extension PaneAppearance {
  /// libghostty's own default colors, for a pane when neither the file nor a renderer says.
  static let defaultTerminal = ChromePalette.Terminal(
    background: RGB(hex: "#282c34") ?? .black, foreground: RGB(hex: "#ffffff") ?? .white)

  /// What the terminal paints with: the config file's `background` and `foreground`, and for one
  /// it left out, what libghostty reads back for it.
  @MainActor
  static var terminal: ChromePalette.Terminal {
    let read = Renderer.current?.terminalColors()
    return ChromePalette.Terminal(
      background: RGB(hex: configuredPane.background) ?? RGB(hex: read?.background)
        ?? defaultTerminal.background,
      foreground: RGB(hex: configuredPane.foreground) ?? RGB(hex: read?.foreground)
        ?? defaultTerminal.foreground)
  }
}

extension ChromePalette {
  @MainActor private static var resolved: [Bool: ChromePalette] = [:]

  /// The palette for a list drawn in `appearance`, worked out once until the colors change.
  ///
  /// Held rather than derived per row: a row is shown again on every state change, and this is
  /// thirty color mixes. Forgotten when the config file is read again (`PaneAppearance.adopt`),
  /// which is the only time the terminal's colors or the agent colors move.
  @MainActor
  public static func current(for appearance: NSAppearance) -> ChromePalette {
    let dark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
    if let held = resolved[dark] { return held }
    var background = RGB.black
    var foreground = RGB.white
    var agents: [String: RGB] = [:]
    var accent = ListAppearance(background: .black, foreground: .white).accent
    appearance.performAsCurrentDrawingAppearance {
      background = RGB(NSColor.windowBackgroundColor)
      foreground = RGB(NSColor.labelColor)
      accent = RGB(NSColor.controlAccentColor)
      for state in ChromePalette.states {
        agents[state] = RGB(PaneAppearance.borderColor(state: state))
      }
    }
    let palette = make(
      terminal: PaneAppearance.terminal, agents: agents,
      appearance: ListAppearance(background: background, foreground: foreground, accent: accent))
    resolved[dark] = palette
    return palette
  }

  @MainActor
  static func forgetResolved() {
    resolved = [:]
  }
}

extension RGB {
  /// An `NSColor` as sRGB components, in whichever appearance is current when this is called.
  init(_ color: NSColor) {
    let srgb = color.usingColorSpace(.sRGB) ?? NSColor.gray.usingColorSpace(.sRGB)!
    self.init(
      red: Double(srgb.redComponent), green: Double(srgb.greenComponent),
      blue: Double(srgb.blueComponent))
  }

  public var color: NSColor {
    NSColor(srgbRed: red, green: green, blue: blue, alpha: 1)
  }
}
