import Testing

@testable import MusterMac

/// Against the platform's real font list rather than a stand-in, on the same terms as the suite
/// spawning a real daemon: what is being judged is whether Muster agrees with CoreText about
/// what is installed, and a stand-in would be Muster's own guess about that agreeing with
/// itself.
///
/// The families here are the ones macOS has shipped for as long as it has been macOS. A machine
/// without Menlo or Helvetica would fail these, and that failure would be worth knowing about.
@Suite("what this machine has to paint with")
struct InstalledFontTests {
  @Test("a monospace family every Mac has")
  func menlo() {
    let found = InstalledFont.look(up: "Menlo")
    #expect(found.found)
    #expect(found.monospaced)
  }

  /// The case a string comparison against a list of family names gets wrong. CoreText resolves a
  /// name written in any case and so does the renderer, so warning here would put a box over a
  /// window painting exactly what was asked for.
  @Test("a name written in another case is the same family")
  func caseInsensitive() {
    let found = InstalledFont.look(up: "menlo")
    #expect(found.found)
    #expect(found.monospaced)
  }

  /// Installed and wrong is a different answer from missing, and the two want different words:
  /// this one renders, and what goes wrong is that the columns stop lining up.
  @Test("a real family that is not monospaced says so")
  func proportional() {
    let found = InstalledFont.look(up: "Helvetica")
    #expect(found.found)
    #expect(!found.monospaced)
  }

  @Test("a family nobody has")
  func missing() {
    #expect(!InstalledFont.look(up: "Fira Cod Not A Font").found)
  }

  /// Not a question. `[font] family` absent means the renderer's own default, which is the
  /// design - so nothing is looked up and nothing is claimed to be missing.
  @Test("naming nothing is not a lookup")
  func nothingNamed() {
    let found = InstalledFont.look(up: "")
    #expect(!found.found)
    #expect(!found.monospaced)
  }

  /// A PostScript name is not a family name, and the renderer would not find one either
  /// (`src/font/discovery.zig` keys on the family-name attribute). Pinned because `Menlo-Regular`
  /// is a plausible thing to write, and Muster saying nothing about it would leave somebody
  /// wondering why their font did not take.
  @Test("a font's own name is not its family's")
  func postScriptName() {
    #expect(!InstalledFont.look(up: "Menlo-Regular").found)
  }
}
