import AppKit

/// What a window with no panes in it says.
///
/// The state is ordinary and was unreachable-looking: closing the last pane leaves a window
/// that renders nothing, and every action except this one is about a pane. So the window says
/// which key makes a pane, read from what that key is actually bound to rather than written
/// down here - a hint naming ⌘T in a window where ⌘T is somebody's own chord for something
/// else is worse than no hint.
///
/// Pure, and separate from the view for the same reason `PaneAppearance` and `SidebarModel`
/// are: the wording and the chord are the decisions, and a decision inside `layout` is one no
/// test can reach.
@MainActor
public enum EmptyWindow {
  public struct Message: Equatable {
    /// What is going on, in as few words as it takes.
    public let headline: String

    /// The way out of it, spelled for the keyboard this window is bound to.
    public let hint: String

    public init(headline: String, hint: String) {
      self.headline = headline
      self.hint = hint
    }
  }

  /// The action that gets a pane back, named the way the rest of the app names it.
  ///
  /// A tab rather than a split, because a split needs a pane to split and there is none. What
  /// the core does with it is make a workspace, which is herdr's word rather than a user's -
  /// the menu says New Tab, so this does too.
  static let recovery = "new_tab"

  public static func message(bindings: [Core.Binding]) -> Message {
    let chord = bindings.first { $0.action == recovery }.map(Shortcuts.spell) ?? ""
    let described = MenuActions.byName[recovery]
    guard !chord.isEmpty else {
      // Unbinding an action is a supported thing to do, so this is a real window rather than
      // a defensive branch: the menu item is still there and still works, and naming the menu
      // it is under is the only true instruction left.
      guard let described else { return Message(headline: headline, hint: "") }
      return Message(
        headline: headline, hint: "Open one from \(described.title), in the \(menu(described)).")
    }
    return Message(headline: headline, hint: "Press \(chord) to open one.")
  }

  private static let headline = "No panes open."

  private static func menu(_ described: MenuActions.Described) -> String {
    "\(described.group.rawValue) menu"
  }
}

/// The message, centered in the space the panes would be in.
///
/// Held by the window and shown late. A cold launch publishes an empty view for the fifty
/// milliseconds it takes the daemon to answer with the workspace Muster just asked it for, and
/// a window that painted this immediately would flash two lines of text on the way up - which
/// reads as a glitch rather than as an explanation.
@MainActor
final class EmptyWindowView: NSView {
  private let headline = NSTextField(labelWithString: "")
  private let hint = NSTextField(labelWithString: "")

  /// How long a window has to stay empty before it is worth saying so.
  static let settle: TimeInterval = 0.25

  /// Bumped every time the window changes its mind, so a delayed reveal that has been
  /// overtaken by a pane arriving does nothing when it fires.
  private var generation = 0

  override init(frame: NSRect) {
    super.init(frame: frame)
    isHidden = true
    headline.font = .systemFont(ofSize: 13, weight: .regular)
    headline.textColor = .secondaryLabelColor
    headline.alignment = .center
    hint.font = .systemFont(ofSize: 12, weight: .regular)
    hint.textColor = .tertiaryLabelColor
    hint.alignment = .center
    addSubview(headline)
    addSubview(hint)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  func apply(_ message: EmptyWindow.Message) {
    headline.stringValue = message.headline
    hint.stringValue = message.hint
    hint.isHidden = message.hint.isEmpty
    needsLayout = true
  }

  /// Whether the window has anything to show, which is the opposite of what this view is for.
  func apply(showing: Bool) {
    generation += 1
    guard !showing else {
      isHidden = true
      return
    }
    let asked = generation
    DispatchQueue.main.asyncAfter(deadline: .now() + EmptyWindowView.settle) { [weak self] in
      guard let self, self.generation == asked else { return }
      self.isHidden = false
    }
  }

  override var isFlipped: Bool { true }

  static let gap: CGFloat = 6

  override func layout() {
    super.layout()
    let first = headline.fittingSize
    let second = hint.isHidden ? .zero : hint.fittingSize
    let total = first.height + (hint.isHidden ? 0 : EmptyWindowView.gap + second.height)
    var y = ((bounds.height - total) / 2).rounded()
    headline.frame = CGRect(x: 0, y: y, width: bounds.width, height: first.height)
    y += first.height + EmptyWindowView.gap
    hint.frame = CGRect(x: 0, y: y, width: bounds.width, height: second.height)
  }
}
