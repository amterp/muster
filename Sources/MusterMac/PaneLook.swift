import AppKit
import SwiftUI

/// The colors the group conversation and the message banner are drawn in.
///
/// Both sit in the pane area - one replaces the panes, the other floats over them - so they take
/// the terminal's background and foreground (Fencil `chrome-colors`) rather than the platform's.
/// SwiftUI's own `.secondary` and system controls follow the macOS appearance instead, and a dark
/// terminal under a light-mode Mac would draw text nobody can read; so every text color here is
/// the palette's, checked at 4.5:1 against the surface it is on, and `colorScheme` is the
/// terminal's lightness so that the controls SwiftUI draws itself agree with it.
struct PaneLook: Equatable {
  let surface: ChromePalette.Surface

  /// Whether the terminal is dark, which is what the system controls should be told.
  var isDark: Bool {
    surface.background.contrast(with: .white) > surface.background.contrast(with: .black)
  }

  var colorScheme: ColorScheme { isDark ? .dark : .light }

  var background: Color { Color(nsColor: surface.background.color) }
  var raised: Color { Color(nsColor: surface.raised.color) }
  var text: Color { Color(nsColor: surface.text.color) }
  var dimText: Color { Color(nsColor: surface.dimText.color) }
  /// A hairline between regions, and the edge of a key.
  var rule: Color { Color(nsColor: surface.keycapEdge.color) }
  var accent: Color { Color(nsColor: surface.accentText.color) }
  var accentTint: Color { Color(nsColor: surface.accentTint.color) }
  var keycapFill: Color { Color(nsColor: surface.keycapFill.color) }

  /// A state's word, readable on this surface.
  func stateText(_ state: String) -> Color {
    Color(nsColor: (surface.stateText[state] ?? surface.dimText).color)
  }

  /// What is drawn before anything has said otherwise: the renderer's own default colors.
  static let standard = PaneLook(
    surface: ChromePalette.make(
      terminal: PaneAppearance.defaultTerminal, agents: [:],
      appearance: .init(background: .black, foreground: .white)
    ).pane)
}

private struct PaneLookKey: EnvironmentKey {
  static let defaultValue = PaneLook.standard
}

extension EnvironmentValues {
  var paneLook: PaneLook {
    get { self[PaneLookKey.self] }
    set { self[PaneLookKey.self] = newValue }
  }
}

/// The look as the window last worked it out, and what tells the views to draw it again when the
/// config file or the appearance changes it.
@MainActor
final class PaneLookBoard: ObservableObject {
  static let shared = PaneLookBoard()

  @Published private(set) var look = PaneLook.standard

  init() {
    refresh()
    // The accent is the system's, and moves without Muster's own colors having changed.
    NotificationCenter.default.addObserver(
      forName: NSColor.systemColorsDidChangeNotification, object: nil, queue: .main
    ) { [weak self] _ in
      MainActor.assumeIsolated {
        ChromePalette.forgetResolved()
        self?.refresh()
      }
    }
  }

  /// Takes the palette for the app's appearance, after the colors may have moved.
  func refresh() {
    let appearance = NSApp?.effectiveAppearance ?? .currentDrawing()
    let fresh = PaneLook(surface: ChromePalette.current(for: appearance).pane)
    if fresh != look { look = fresh }
  }
}

/// Puts a view in the terminal's colors: the background, the text color, the scheme, and the look
/// in the environment for the views inside to read.
struct InPaneLook: ViewModifier {
  @ObservedObject var board: PaneLookBoard
  @Environment(\.colorScheme) private var systemScheme

  func body(content: Content) -> some View {
    content
      .environment(\.paneLook, board.look)
      .environment(\.colorScheme, board.look.colorScheme)
      .foregroundStyle(board.look.text)
      // The macOS appearance changing moves the accent, so the look is worked out again.
      .onChange(of: systemScheme) { _, _ in board.refresh() }
  }
}

extension View {
  func inPaneLook(_ board: PaneLookBoard = .shared) -> some View {
    modifier(InPaneLook(board: board))
  }
}
