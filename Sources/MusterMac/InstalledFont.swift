import CoreText
import Foundation

/// Whether this machine has the font family somebody named, and whether it is monospaced.
///
/// The one appearance value the core cannot answer for itself: which fonts a machine has is a
/// platform question, so this reports and `muster_core::font` decides what it means. Nothing
/// here has an opinion about what to do with the answer.
///
/// Resolved rather than compared against a list of families, for two reasons. It is cheap - a
/// lookup is under a millisecond once CoreText is warm, where enumerating every family is over a
/// tenth of a second on a launch. And it is the same question the renderer asks: ghostty builds
/// a descriptor keyed on the family-name attribute and collects what matches
/// (`src/font/discovery.zig`), so a name written in another case resolves here exactly as it
/// will there. A string comparison would report `menlo` as missing and warn about a font that is
/// about to paint perfectly well.
public enum InstalledFont {
  public struct Match: Sendable {
    public let found: Bool
    /// Meaningless when `found` is false: there is no font to have the trait.
    public let monospaced: Bool
  }

  /// What the platform makes of a family name.
  ///
  /// An empty name is not a question - `[font] family` absent means the renderer's own default,
  /// which is the design rather than something to check.
  public static func look(up family: String) -> Match {
    guard !family.isEmpty else { return Match(found: false, monospaced: false) }

    let wanted = CTFontDescriptorCreateWithAttributes(
      [kCTFontFamilyNameAttribute: family] as CFDictionary)
    let matched = CTFontDescriptorCreateMatchingFontDescriptor(
      wanted, Set([kCTFontFamilyNameAttribute as String]) as CFSet)
    guard let matched else { return Match(found: false, monospaced: false) }

    // The trait off the descriptor rather than off an instantiated font, which is the same
    // answer without building one.
    let traits = CTFontDescriptorCopyAttribute(matched, kCTFontTraitsAttribute) as? [String: Any]
    let symbolic = traits?[kCTFontSymbolicTrait as String] as? UInt32 ?? 0
    let monospaced = symbolic & UInt32(CTFontSymbolicTraits.traitMonoSpace.rawValue) != 0
    return Match(found: true, monospaced: monospaced)
  }
}
