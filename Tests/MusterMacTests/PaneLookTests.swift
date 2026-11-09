import SwiftUI
import Testing

@testable import MusterMac

@MainActor
@Suite("the conversation and the banner take the terminal's colors")
struct PaneLookTests {
  @Test("the look follows the terminal, and a reload of the colors moves it")
  func followsTheTerminal() {
    defer { PaneAppearance.adopt(chrome: .none) }
    PaneAppearance.adopt(chrome: .none, pane: .init(background: "#101010", foreground: "#f0f0f0"))
    let board = PaneLookBoard()
    #expect(board.look.isDark)
    #expect(board.look.surface.background.hex == "#101010")

    PaneAppearance.adopt(chrome: .none, pane: .init(background: "#fdf6e3", foreground: "#073642"))
    board.refresh()
    #expect(!board.look.isDark, "a light terminal under a dark-mode Mac is drawn light")
    #expect(board.look.surface.background.hex == "#fdf6e3")
    #expect(board.look.colorScheme == .light)
  }

  @Test("its text reads on its own background, whatever the Mac's appearance")
  func readable() {
    for (background, foreground) in [("#101010", "#f0f0f0"), ("#fdf6e3", "#657b83")] {
      defer { PaneAppearance.adopt(chrome: .none) }
      PaneAppearance.adopt(
        chrome: .none, pane: .init(background: background, foreground: foreground))
      let surface = PaneLookBoard().look.surface
      for text in [surface.text, surface.dimText, surface.accentText] {
        #expect(text.contrast(with: surface.background) >= ChromePalette.minimumContrast)
      }
    }
  }
}
