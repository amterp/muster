import AppKit
import Testing

@testable import MusterMac

// The redesign's colors are computed, so what can go wrong is a computed color that cannot be
// read on somebody's theme. Each case here runs the one pure function against a theme and checks
// the floor for every text and surface it can pair, rather than eyeballing a screenshot of the
// two themes that happened to be to hand.

@Suite("the chrome palette keeps its text readable on any theme")
struct ChromePaletteTests {
  private func rgb(_ hex: String) -> RGB { RGB(hex: hex)! }

  private struct Theme {
    let name: String
    let terminal: ChromePalette.Terminal
  }

  /// A dark theme, a light one whose foreground is only just readable on its own background
  /// (Solarized light's is about 4:1), and the renderer's own default.
  private var themes: [Theme] {
    [
      Theme(
        name: "dark",
        terminal: .init(background: rgb("#1e1e2e"), foreground: rgb("#cdd6f4"))),
      Theme(
        name: "light",
        terminal: .init(background: rgb("#fdf6e3"), foreground: rgb("#657b83"))),
      Theme(name: "renderer default", terminal: PaneAppearance.defaultTerminal),
    ]
  }

  /// The agent list is the platform's, so it is dark or light on its own account, whatever the
  /// terminal beside it is - which is the pairing a computed color is most likely to lose.
  private var lists: [(name: String, list: ChromePalette.ListAppearance, agents: [String: RGB])] {
    [
      (
        "dark list",
        .init(background: rgb("#1e1f23"), foreground: rgb("#ececef")),
        [
          "working": rgb("#4cc3e8"), "blocked": rgb("#ff9f0a"), "stuck": rgb("#ff5fae"),
          "waiting": rgb("#a5a3ff"), "done": rgb("#3dd16b"), "idle": rgb("#9497a1"),
          "unknown": rgb("#9497a1"),
        ]
      ),
      (
        "light list",
        .init(background: rgb("#f2f2f4"), foreground: rgb("#1d1d1f")),
        [
          "working": rgb("#0b8fbf"), "blocked": rgb("#ef8100"), "stuck": rgb("#d6337f"),
          "waiting": rgb("#5b57d1"), "done": rgb("#1fa34a"), "idle": rgb("#8a8e96"),
          "unknown": rgb("#8a8e96"),
        ]
      ),
    ]
  }

  @Test(
    "every text on every surface it can sit on reaches 4.5:1, for every theme beside every list")
  func contrastFloor() {
    var checked = 0
    for theme in themes {
      for list in lists {
        let palette = ChromePalette.make(
          terminal: theme.terminal, agents: list.agents, appearance: list.list)
        for (surfaceName, surface) in [("list", palette.list), ("header", palette.header)] {
          let context = "\(theme.name) terminal, \(list.name), \(surfaceName)"
          let floor = ChromePalette.minimumContrast
          for (name, text) in [("text", surface.text), ("dim text", surface.dimText)] {
            for (under, behind) in [("background", surface.background), ("raised", surface.raised)]
            {
              #expect(
                text.contrast(with: behind) >= floor, "\(name) on \(under): \(context)")
              checked += 1
            }
          }
          for state in ChromePalette.states {
            let word = surface.stateText[state]!
            for (under, behind) in [
              ("background", surface.background), ("raised", surface.raised),
              ("its tint", surface.tint[state]!),
            ] {
              #expect(
                word.contrast(with: behind) >= floor, "\(state) word on \(under): \(context)")
              checked += 1
            }
          }
          #expect(
            surface.keycapText.contrast(with: surface.keycapFill) >= floor,
            "keycap digit: \(context)")
          checked += 1
        }
      }
    }
    #expect(checked > 100, "so few pairs were checked that a loop must have run empty")
  }

  @Test("a tint is the state's color mixed into the surface, so repainting the state repaints it")
  func tintFollowsTheAgentColor() {
    let list = lists[0]
    func tint(blocked: String) -> RGB {
      var agents = list.agents
      agents["blocked"] = rgb(blocked)
      return ChromePalette.make(terminal: themes[0].terminal, agents: agents, appearance: list.list)
        .list.tint["blocked"]!
    }
    let orange = tint(blocked: "#ff9f0a")
    let green = tint(blocked: "#00ff00")

    #expect(orange != green)
    #expect(orange.red > orange.blue, "an orange tint is warm")
    #expect(green.green > green.red, "a green one is not")
    // Still the surface, mostly: a tint behind text, not a fill.
    #expect(orange.contrast(with: list.list.background) < 1.5)
  }

  @Test("the header is the terminal's background moved a little toward its foreground")
  func headerIsNearTheTerminal() {
    for theme in themes {
      let palette = ChromePalette.make(
        terminal: theme.terminal, agents: lists[0].agents, appearance: lists[0].list)
      let header = palette.header.background
      #expect(header != theme.terminal.background, "\(theme.name)")
      #expect(
        header.contrast(with: theme.terminal.background) < 1.3,
        "\(theme.name): a header that stands apart from its pane is not part of it")
    }
  }

  @Test("text whose own foreground cannot be read goes to black or white instead")
  func unreadableForegroundFallsBack() {
    let murky = ChromePalette.Terminal(background: rgb("#808080"), foreground: rgb("#888888"))
    let palette = ChromePalette.make(
      terminal: murky, agents: lists[0].agents, appearance: lists[0].list)

    #expect(palette.header.text.contrast(with: palette.header.background) >= 4.5)
    #expect(palette.header.dimText.contrast(with: palette.header.background) >= 4.5)
  }

  @Test("hex round-trips and contrast spans 1 to 21")
  func arithmetic() {
    #expect(rgb("#3dd16b").hex == "#3dd16b")
    #expect(RGB(hex: "3dd16b") != nil)
    #expect(RGB(hex: "#12345") == nil)
    #expect(RGB(hex: nil) == nil)
    #expect(abs(RGB.black.contrast(with: .white) - 21) < 0.001)
    #expect(abs(RGB.white.contrast(with: .white) - 1) < 0.001)
  }

  @MainActor
  @Test("the terminal's colors are the file's when it set them")
  func terminalFromTheFile() {
    defer { PaneAppearance.adopt(chrome: .none) }
    PaneAppearance.adopt(
      chrome: .none,
      pane: .init(background: "#112233", foreground: "#ddeeff"))

    #expect(PaneAppearance.terminal.background.hex == "#112233")
    #expect(PaneAppearance.terminal.foreground.hex == "#ddeeff")
  }

  @MainActor
  @Test("a palette is held until the colors change, and worked out again after")
  func paletteIsHeldUntilAdopt() {
    defer { PaneAppearance.adopt(chrome: .none) }
    let dark = NSAppearance(named: .darkAqua)!
    PaneAppearance.adopt(chrome: .none, pane: .init(background: "#101010", foreground: "#f0f0f0"))
    let before = ChromePalette.current(for: dark)

    PaneAppearance.adopt(chrome: .none, pane: .init(background: "#202080", foreground: "#f0f0f0"))
    let after = ChromePalette.current(for: dark)

    #expect(before.header.background != after.header.background)
    #expect(
      after.list == before.list, "the list is the platform's and the terminal did not move it")
  }
}
