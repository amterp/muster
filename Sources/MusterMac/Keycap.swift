import AppKit

/// One digit of a chord, drawn as the key it names.
///
/// A row carries the whole chord as two of these at full contrast (Fencil `row-whole-chord`): a
/// digit in the secondary color a step below its name was too faint to find at a glance, which is
/// what a chord is for. Shared by the agent list, the Needs you strip and pane headers, so the
/// chord looks the same wherever it is drawn.
///
/// Two layers, an edge behind a face that sits a point above its bottom, which is what makes it
/// read as a key rather than a bordered label.
@MainActor
final class KeycapView: NSView {
  static let width: CGFloat = 15
  static let height: CGFloat = 17

  private let edge = CALayer()
  private let face = CALayer()
  private let digit = NSTextField(labelWithString: "")

  /// What the key is painted in, from the surface it sits on. Kept for the tests and for a repaint.
  private(set) var faceColor = NSColor.clear
  private(set) var edgeColor = NSColor.clear
  private(set) var isLive = false

  init(digit: Int) {
    super.init(frame: CGRect(x: 0, y: 0, width: Self.width, height: Self.height))
    wantsLayer = true
    layer?.addSublayer(edge)
    layer?.addSublayer(face)
    edge.cornerRadius = 4
    face.cornerRadius = 3
    self.digit.font = .monospacedDigitSystemFont(ofSize: 10.5, weight: .semibold)
    self.digit.alignment = .center
    addSubview(self.digit)
    self.digit.stringValue = String(digit)
    setAccessibilityLabel("key \(digit)")
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  override var fittingSize: NSSize { NSSize(width: Self.width, height: Self.height) }

  /// Paints it from `surface`. `live` is the press the next keystroke can make, drawn with the
  /// accent as its edge: at that moment it is not a reference somebody might consult but the
  /// thing the hand is about to do.
  func show(digit: Int, live: Bool, surface: ChromePalette.Surface) {
    self.digit.stringValue = String(digit)
    isLive = live
    faceColor = surface.keycapFill.color
    edgeColor = live ? NSColor.controlAccentColor : surface.keycapEdge.color
    self.digit.textColor = surface.keycapText.color
    setAccessibilityLabel("key \(digit)")
    resolveColors()
  }

  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    resolveColors()
  }

  private func resolveColors() {
    effectiveAppearance.performAsCurrentDrawingAppearance {
      face.backgroundColor = faceColor.cgColor
      edge.backgroundColor = edgeColor.cgColor
    }
  }

  override func layout() {
    super.layout()
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    edge.frame = bounds
    // The face is the key's top, a point in from the sides and the top and two above the bottom
    // edge, which is the lip.
    face.frame = CGRect(x: 1, y: 2, width: bounds.width - 2, height: bounds.height - 3)
    CATransaction.commit()
    let text = digit.fittingSize
    digit.frame = CGRect(
      x: 0, y: face.frame.midY - text.height / 2, width: bounds.width, height: text.height)
  }
}
