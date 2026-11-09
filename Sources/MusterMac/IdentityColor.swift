import Foundation

/// The color an agent's avatar is drawn in, worked out from who it is and nothing else.
///
/// It comes from the participant's identity - the pane id for an agent in a pane, which is never
/// reused, and the name for a participant that joined by name - hashed into a fixed palette. So
/// an agent is the same color in every group, window and launch, with nothing stored. It appears
/// on avatars in conversations and banners only: in the agent list a color means a state (Fencil
/// `agent-identity-colors`).
///
/// The eight are dark enough for a white initial to reach 4.5:1 and are kept apart from the
/// seven state colors and the six machine colors the app ships, so an avatar is never read as a
/// state. Two members of a group can share one; that is accepted over giving an agent a different
/// color in each group.
enum IdentityColor {
  static let palette: [RGB] = [
    "#0f766e", "#1d4ed8", "#9f1239", "#6d28d9", "#b45309", "#15803d", "#a21caf", "#0e7490",
  ].map { RGB(hex: $0)! }

  /// The person, and Muster itself: fixed, and outside the palette.
  static let human = RGB(hex: "#3f3f46")!
  static let muster = RGB(hex: "#57534e")!

  /// What a participant is hashed by: the pane id for a pane, else the name.
  static func identity(paneID: String, name: String) -> String {
    paneID.isEmpty ? name : paneID
  }

  static func color(identity: String) -> RGB {
    palette[Int(fnv1a(Array(identity.utf8)) % UInt32(palette.count))]
  }

  /// The avatar color for an author or member.
  static func color(paneID: String, name: String, isHuman: Bool = false, isMuster: Bool = false)
    -> RGB
  {
    if isMuster { return muster }
    if isHuman { return human }
    return color(identity: identity(paneID: paneID, name: name))
  }

  /// FNV-1a over 32 bits. Written out because `hashValue` is seeded per process, and a color that
  /// changed on every launch would not be an identity.
  static func fnv1a(_ bytes: [UInt8]) -> UInt32 {
    bytes.reduce(0x811c_9dc5) { ($0 ^ UInt32($1)) &* 0x0100_0193 }
  }
}
