import AppKit

/// What a pane's agent state looks like, and what the window says about the daemon.
///
/// Pure and separate from the view, because these are the decisions - which state gets
/// which color, what a stale backend does to a title - and a decision inside `draw` is a
/// decision no test can reach. The views below only apply what this returns.
public enum PaneAppearance {
  /// The border color for an agent state, by the backend's own spelling.
  ///
  /// Anything unrecognized is `unknown`'s gray rather than a default of `idle`'s green.
  /// A state we could not read is not an agent that finished, and coloring it as one is
  /// how a user learns to stop trusting the colors.
  public static func borderColor(state: String) -> NSColor {
    switch state {
    case "working": return NSColor.systemBlue
    case "blocked": return NSColor.systemOrange
    case "done": return NSColor.systemGreen
    case "idle": return NSColor.systemGray
    default: return NSColor.systemGray
    }
  }

  /// Whether a state deserves a visible border at all.
  ///
  /// Idle and unknown do not. Every pane carrying a colored edge all the time is every
  /// pane carrying none: the border exists to be noticed, and it is only noticeable if
  /// the resting state is bare.
  public static func isHighlighted(state: String) -> Bool {
    state == "working" || state == "blocked" || state == "done"
  }

  /// The window title: which pane the keyboard feeds, and whether the daemon is still there.
  ///
  /// Agent state used to be here and is not any more, because a window with fifteen panes has
  /// fifteen agent states and one title - the per-pane borders carry that now. What survives
  /// is the part that is genuinely per-window: a window rendering an hour-old session as
  /// though it were live is the worst failure available to this product, and it is
  /// indistinguishable from a working one without saying so.
  ///
  /// Zoom is here for a different reason: a zoomed tab and a tab with one pane look identical
  /// on screen, so without this a user has no way to learn why their other panes vanished.
  /// `problem` is why there is no pane, when there was meant to be one. A window that asked
  /// for `w9:p99` and got nothing must not be titled as the renderer check, which is what the
  /// same empty state means when nobody named a pane at all - those two look identical on
  /// screen and want opposite reactions.
  public static func title(
    paneID: String?, zoomed: Bool, health: String, detail: String, problem: String? = nil
  ) -> String {
    guard let paneID, !paneID.isEmpty else {
      if let problem { return "muster - \(problem)" }
      return "muster (renderer check - keyboard not connected)"
    }
    var title = "muster - \(paneID)"
    if zoomed {
      title += " · zoomed"
    }
    switch health {
    case "connected", "":
      break
    case "stale":
      title += detail.isEmpty ? " · stale" : " · stale (\(detail))"
    default:
      title += " · \(health)"
    }
    return title
  }
}

/// Holds one surface, draws the agent state around it, and says whether it has the keyboard.
///
/// A container rather than the surface itself, because libghostty attaches its own Metal
/// layer to the view it is handed and a border on that layer is a fight over ownership -
/// not a thing to discover at render time.
///
/// Two edges, not one, and they mean different things. The outer is the agent's state, which
/// is the whole point of the product; the inner says which pane a keystroke would go to. One
/// edge carrying both would make a working pane and a focused pane indistinguishable, which
/// is exactly the confusion these are for clearing up.
@MainActor
public final class PaneChrome: NSView {
  /// The pane this view is showing, or nil for the renderer check.
  public private(set) var paneID: String?
  public private(set) var state: String = "unknown"
  public private(set) var isFocused = false

  public let surface: SurfaceView

  /// Called when somebody clicks this pane, meaning they want the keyboard here.
  public var onFocusRequested: ((String) -> Void)?

  private let focusRing = CALayer()

  public init(frame: NSRect, surface: SurfaceView) {
    self.surface = surface
    super.init(frame: frame)
    wantsLayer = true
    layer?.addSublayer(focusRing)
    // Never animated. A frame set from a layout pass would otherwise slide into place over a
    // quarter second, and a surface whose bounds lag its host by that long renders the
    // previous arrangement while the daemon has already moved on.
    focusRing.actions = ["position": NSNull(), "bounds": NSNull(), "borderColor": NSNull()]
    addSubview(surface)
    surface.onClick = { [weak self] in
      guard let self, let paneID = self.paneID else { return }
      self.onFocusRequested?(paneID)
    }
    applyAppearance()
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  public static let borderWidth: CGFloat = 2

  /// How much of a pane's edge is chrome rather than terminal.
  public static let inset: CGFloat = borderWidth * 2

  public func attach(paneID: String?) {
    self.paneID = paneID
    applyAppearance()
  }

  /// Takes a state change, if it is about this pane.
  ///
  /// The core sends every pane's transitions, because the sidebar wants them and the log
  /// wants them all. A window showing several panes has to filter, and one that forgot to
  /// would show its neighbor's agent as its own.
  public func apply(paneID: String, state: String) {
    guard paneID == self.paneID else { return }
    self.state = state
    applyAppearance()
  }

  public func apply(focused: Bool) {
    guard focused != isFocused else { return }
    isFocused = focused
    applyAppearance()
  }

  public override func layout() {
    super.layout()
    surface.frame = bounds.insetBy(dx: PaneChrome.inset, dy: PaneChrome.inset)
    focusRing.frame = bounds.insetBy(dx: PaneChrome.borderWidth, dy: PaneChrome.borderWidth)
  }

  private func applyAppearance() {
    let highlighted = PaneAppearance.isHighlighted(state: state)
    layer?.borderWidth = highlighted ? PaneChrome.borderWidth : 0
    layer?.borderColor = PaneAppearance.borderColor(state: state).cgColor
    focusRing.borderWidth = PaneChrome.borderWidth
    focusRing.borderColor =
      isFocused ? NSColor.controlAccentColor.cgColor : NSColor.clear.cgColor
    needsLayout = true
  }
}
