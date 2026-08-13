/// Turns a keystroke into the bytes a terminal program expects.
///
/// A protocol so the core can own the input pipeline without owning an encoder: the real
/// one is libghostty-vt's, which lives behind the renderer's own dependency, and a test
/// wants a table it can read. Encoding is the one part of input that must agree exactly
/// with a published implementation, so the seam is here rather than a reimplementation.
public protocol KeyEncoding: AnyObject {
  /// The bytes for this keystroke, or empty when the keystroke produces none - a bare
  /// modifier, or any key while an input method is composing.
  func encode(_ key: KeyEvent) throws -> [UInt8]
}
