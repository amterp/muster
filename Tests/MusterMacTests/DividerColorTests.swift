import AppKit
import Testing

@testable import MusterMac

// The divider is the one piece of the window's appearance no terminal config can reach:
// everything inside a pane is libghostty's, painted from whatever config it finds, and this
// line is Muster's own. Which makes reading the colour the shell's job, and the only part of
// it worth a test - the core already refused anything malformed when it read the file.

@Suite("the divider takes its colour from the config")
struct DividerColorTests {
  @Test("six hex digits become that colour, with or without the hash")
  func hexIsRead() {
    // Both spellings, because the core normalizes to one and a shell that only read that one
    // would break silently the day anything else handed it a colour.
    for spelling in ["#ff5f00", "ff5f00"] {
      let color = NSColor(hex: spelling)
      #expect(color != nil, "\(spelling) was not read as a colour")
      let rgb = color?.usingColorSpace(.deviceRGB)
      #expect(rgb?.redComponent == 1.0)
      #expect(((rgb?.greenComponent ?? 0) - 95.0 / 255.0).magnitude < 0.001)
      #expect(rgb?.blueComponent == 0.0)
    }
  }

  @Test("anything that is not six hex digits is refused rather than approximated")
  func rubbishIsRefused() {
    // Falling back is the caller's decision and it takes the platform's separator. Inventing
    // a colour here would paint a line somebody did not ask for and could not explain.
    for spelling in ["#gg0000", "#ff5f0", "#ff5f000", "", "red", "#ff 5f00"] {
      #expect(NSColor(hex: spelling) == nil, "\(spelling) was read as a colour")
    }
  }

  @Test("the two ends of the range survive the round trip")
  func extremesAreExact() {
    // Black is the value that catches a parser using zero as its failure answer, and white
    // the one that catches an off-by-one in the byte arithmetic.
    let black = NSColor(hex: "#000000")?.usingColorSpace(.deviceRGB)
    #expect(black?.redComponent == 0 && black?.greenComponent == 0 && black?.blueComponent == 0)

    let white = NSColor(hex: "#ffffff")?.usingColorSpace(.deviceRGB)
    #expect(white?.redComponent == 1 && white?.greenComponent == 1 && white?.blueComponent == 1)
  }
}
