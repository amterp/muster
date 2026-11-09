import Testing

@testable import MusterMac

@Suite("an agent's avatar color comes from who it is")
struct IdentityColorTests {
  @Test("one identity is one color, whatever asks and however often")
  func stable() {
    #expect(IdentityColor.color(identity: "w1:p3") == IdentityColor.color(identity: "w1:p3"))
    // A value written down, so a hash seeded per process would fail here on some launch.
    #expect(IdentityColor.fnv1a(Array("a".utf8)) == 0xe40c_292c)
  }

  @Test("a pane is known by its id, and a participant that joined by name by that name")
  func identity() {
    #expect(IdentityColor.identity(paneID: "w1:p3", name: "otter") == "w1:p3")
    #expect(IdentityColor.identity(paneID: "", name: "linen") == "linen")
    #expect(
      IdentityColor.color(paneID: "w1:p3", name: "otter")
        == IdentityColor.color(paneID: "w1:p3", name: "a name since given"),
      "a rename does not change an agent's color")
  }

  @Test("the person and Muster have colors of their own, outside the eight")
  func fixed() {
    #expect(!IdentityColor.palette.contains(IdentityColor.human))
    #expect(!IdentityColor.palette.contains(IdentityColor.muster))
    #expect(IdentityColor.human != IdentityColor.muster)
    #expect(IdentityColor.color(paneID: "", name: "x", isHuman: true) == IdentityColor.human)
    #expect(IdentityColor.color(paneID: "", name: "x", isMuster: true) == IdentityColor.muster)
  }

  @Test("identities spread over the palette rather than landing on one color")
  func spreads() {
    let used = Set((1...40).map { IdentityColor.color(identity: "w1:p\($0)").hex })
    #expect(used.count >= 6)
  }

  @Test("every color carries a white initial at 4.5:1")
  func readableInitial() {
    for color in IdentityColor.palette + [IdentityColor.human, IdentityColor.muster] {
      #expect(color.contrast(with: .white) >= ChromePalette.minimumContrast, "\(color.hex)")
    }
  }

  @Test("no avatar color is near a state color or a machine color")
  func clearOfStateAndMachine() {
    let states = ["#7aa2f7", "#ff9e64", "#ff79c6", "#7c7fd8", "#9ece6a", "#565f89", "#3b4261"]
    let machines = ["#ff4f9a", "#f2c200", "#a8784f", "#b25ce6", "#e5484d", "#8d9440"]
    func distance(_ a: RGB, _ b: RGB) -> Double {
      let (r, g, b2) = (a.red - b.red, a.green - b.green, a.blue - b.blue)
      return (r * r + g * g + b2 * b2).squareRoot() * 255
    }
    for color in IdentityColor.palette {
      for other in (states + machines).compactMap({ RGB(hex: $0) }) {
        #expect(distance(color, other) >= 60, "\(color.hex) is near \(other.hex)")
      }
    }
  }
}
