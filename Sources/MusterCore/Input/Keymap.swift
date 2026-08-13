/// What Muster does with a keystroke before the pane sees it.
///
/// Input precedence is fixed (architecture.md): the keymap gets first refusal on every
/// chord, and only what it declines is reported toward the focused pane. That ordering
/// is structural here from the first keystroke rather than retrofitted later, because
/// retrofitting it means finding every place that already sends bytes.
///
/// There are no bindings yet. The type exists so the call site is right, and so the day
/// a config file arrives it fills this in rather than rearranging the input path.
public struct Keymap: Sendable {
  public init() {}

  /// What a chord resolves to.
  public enum Resolution: Equatable, Sendable {
    /// Muster handles it; the pane never sees it.
    case action(Action)
    /// Not bound. Report it to the pane.
    case unbound
  }

  /// The operations a chord can be bound to.
  ///
  /// Empty until there is something to bind. Kept as a type rather than a comment so the
  /// dispatcher has somewhere to grow, and so `Resolution` is not a lie about a shape
  /// that does not exist.
  public enum Action: Equatable, Sendable {}

  public func resolve(_ key: KeyEvent) -> Resolution {
    .unbound
  }
}
