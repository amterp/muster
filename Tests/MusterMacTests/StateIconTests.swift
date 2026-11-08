import AppKit
import Testing

@testable import MusterMac

// A state has to read without its color: that is the point of giving each its own shape. These
// render the real layers to a bitmap and compare what was drawn, since a path that is wrong looks
// like an icon that is merely a little odd.

@Suite("state icons and keycaps")
@MainActor
struct StateIconTests {
  private let states = [
    "working", "blocked", "stuck", "waiting", "done", "idle", "unknown", "unreachable",
  ]
  private let pixels = 56

  /// A state's icon as alpha values, row by row from the top, drawn at four pixels to a point.
  private func alpha(_ state: String) -> [UInt8] {
    let icon = StateIcon()
    icon.show(state: state, color: .white)
    let space = CGColorSpaceCreateDeviceRGB()
    let context = CGContext(
      data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: pixels * 4,
      space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.scaleBy(x: 4, y: 4)
    icon.render(in: context)
    let data = context.data!.assumingMemoryBound(to: UInt8.self)
    return (0..<pixels * pixels).map { data[$0 * 4 + 3] }
  }

  /// The alpha at a point of the 16-unit artwork the shapes are drawn in.
  private func alpha(_ state: String, atArtwork x: Double, _ y: Double) -> UInt8 {
    let drawn = alpha(state)
    return drawn[Int(y * 3.5) * pixels + Int(x * 3.5)]
  }

  private func coverage(_ state: String) -> Double {
    let drawn = alpha(state)
    return drawn.reduce(0) { $0 + Double($1) } / (255 * Double(drawn.count))
  }

  @Test("no two states are drawn alike")
  func shapesAreDistinct() {
    let drawn = states.map { alpha($0) }
    for first in states.indices {
      for second in states.indices where second > first {
        let differing = zip(drawn[first], drawn[second]).filter { abs(Int($0) - Int($1)) > 64 }
        #expect(
          Double(differing.count) / Double(pixels * pixels) > 0.02,
          "\(states[first]) and \(states[second]) are nearly the same shape")
      }
    }
  }

  @Test("the states that want somebody are solid and the calm ones are outlines")
  func filledAndOutlined() {
    for state in states {
      let covered = coverage(state)
      if StateIcon.isFilled(state: state) {
        #expect(covered > 0.45, "\(state) should be a solid shape, covering \(covered)")
      } else {
        #expect(covered < 0.35, "\(state) should be an outline, covering \(covered)")
      }
    }
    #expect(states.filter(StateIcon.isFilled).sorted() == ["blocked", "done", "stuck"])
  }

  @Test("the marks in a solid shape are cut out of it, so the tint shows through")
  func marksAreCutOut() {
    // The exclamation bar, the bar across the octagon and the check, each in artwork units.
    #expect(alpha("blocked", atArtwork: 8, 6) == 0)
    #expect(alpha("stuck", atArtwork: 8, 8) == 0)
    #expect(alpha("done", atArtwork: 7, 10.7) == 0)
    // And beside each one it is solid.
    for state in ["blocked", "stuck", "done"] {
      #expect(alpha(state, atArtwork: 3.2, 8) > 200, "\(state)")
    }
  }

  @Test("a state nobody has heard of is drawn as unknown")
  func unrecognizedIsUnknown() {
    #expect(alpha("never-heard-of-it") == alpha("unknown"))
  }

  @Test("a keycap takes its fill and edge from the surface, and the live one an accent edge")
  func keycapColors() {
    let palette = ChromePalette.make(
      terminal: PaneAppearance.defaultTerminal,
      agents: [:],
      appearance: .init(background: RGB(hex: "#1e1f23")!, foreground: RGB(hex: "#ececef")!))
    let key = KeycapView(digit: 2)
    key.show(digit: 3, live: false, surface: palette.list)
    let rest = key.edgeColor

    #expect(key.faceColor == palette.list.keycapFill.color)
    #expect(rest == palette.list.keycapEdge.color)
    #expect(!key.isLive)

    key.show(digit: 3, live: true, surface: palette.list)

    #expect(key.isLive)
    #expect(key.edgeColor != rest)
    #expect(key.fittingSize == NSSize(width: KeycapView.width, height: KeycapView.height))
  }
}
