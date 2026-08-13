/// What Muster wants to happen in a pane, in Muster's own words.
///
/// The core does not know herdr exists. It produces intents; an adapter turns them into
/// whatever the backend of the day speaks (architecture.md, the backend seam). Keeping the
/// vocabulary here rather than reusing herdr's wire types is what makes the adapter a
/// translation rather than a passthrough, and what lets a test assert on intent without a
/// daemon.
public enum PaneIntent: Equatable, Sendable {
  /// Bytes for the pane's PTY, already encoded.
  case input([UInt8])

  /// Text for the pane, left for the backend to encode.
  ///
  /// Distinct from `input` because the backend can do this better than we can: it knows
  /// the pane's real terminal modes, so it fences a paste correctly where Muster would
  /// have to guess (`docs/observations/herdr-0.8.0.md` section 5).
  case text(String)

  /// A named key, left for the backend to encode against the pane's real modes.
  ///
  /// The escape hatch for the keys where guessing is known to be wrong - the arrows above
  /// all, since a program that called `smkx` wants `SS3` and one that did not wants `CSI`,
  /// and nothing on the control stream says which.
  case key(name: String)

  /// A wheel movement, which the backend routes against the pane's mouse mode.
  case scroll(direction: ScrollDirection, lines: UInt16)

  public enum ScrollDirection: String, Equatable, Sendable {
    case up
    case down
  }
}

/// Where a pane's intents go.
///
/// Two implementations exist and they differ in a way the core must not care about: one
/// writes bytes onto a control stream the pane already holds open, the other asks the
/// daemon to encode and costs a round trip. `deliver` returning whether it arrived is what
/// lets the caller degrade instead of silently swallowing input.
public protocol PaneChannel: AnyObject {
  /// Sends one intent, and says whether it got there.
  func deliver(_ intent: PaneIntent) -> Bool

  /// Whether this channel can encode an intent the client cannot - text and named keys.
  ///
  /// A control stream alone cannot: it is a raw pipe to the PTY, so anything sent on it
  /// has already been encoded by us, against a guess.
  var encodesServerSide: Bool { get }

  /// Named for logs, so a failure says which channel dropped the input.
  var channelDescription: String { get }
}
