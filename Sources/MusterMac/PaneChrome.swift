import AppKit

/// What a pane's agent state looks like, and what the window says about the daemon.
///
/// Pure and separate from the view, because these are the decisions - which state gets
/// which color, what a stale backend does to a title - and a decision inside `draw` is a
/// decision no test can reach. The view below only applies what this returns.
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

  /// The window title, which is where a stale backend has to be admitted.
  ///
  /// A window rendering an hour-old session as though it were live is the worst failure
  /// available to this product, and it is indistinguishable from a working one without
  /// this. Connected says nothing extra - a title that always carries a status word is a
  /// title nobody reads.
  public static func title(paneID: String?, state: String, health: String, detail: String)
    -> String
  {
    guard let paneID else { return "muster (renderer check - keyboard not connected)" }
    var title = "muster - \(paneID)"
    if PaneAppearance.isHighlighted(state: state) {
      title += " · \(state)"
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

/// Holds one surface and draws the agent state around it.
///
/// A container rather than the surface itself, because libghostty attaches its own Metal
/// layer to the view it is handed and a border on that layer is a fight over ownership -
/// not a thing to discover at render time.
@MainActor
public final class PaneChrome: NSView {
  /// The pane this window is showing, or nil for the renderer check.
  public private(set) var paneID: String?
  public private(set) var state: String = "unknown"
  public private(set) var health: String = "disconnected"
  public private(set) var detail: String = ""

  private let surface: SurfaceView

  public init(frame: NSRect, surface: SurfaceView) {
    self.surface = surface
    super.init(frame: frame)
    wantsLayer = true
    surface.frame = bounds.insetBy(dx: PaneChrome.borderWidth, dy: PaneChrome.borderWidth)
    surface.autoresizingMask = [.width, .height]
    addSubview(surface)
    applyAppearance()
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  public static let borderWidth: CGFloat = 2

  public func attach(paneID: String?) {
    self.paneID = paneID
    applyAppearance()
  }

  /// Takes a state change, if it is about this pane.
  ///
  /// The core sends every pane's transitions, because the sidebar wants them and the log
  /// wants them all. A window showing one pane has to filter, and a window that forgot to
  /// would show its neighbor's agent as its own.
  public func apply(paneID: String, state: String) {
    guard paneID == self.paneID else { return }
    self.state = state
    applyAppearance()
  }

  public func apply(health: String, detail: String) {
    self.health = health
    self.detail = detail
    applyAppearance()
  }

  private func applyAppearance() {
    let highlighted = PaneAppearance.isHighlighted(state: state)
    layer?.borderWidth = highlighted ? PaneChrome.borderWidth : 0
    layer?.borderColor = PaneAppearance.borderColor(state: state).cgColor
    window?.title = PaneAppearance.title(
      paneID: paneID, state: state, health: health, detail: detail)
  }

  public override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    // The title is set from here as well as from every state change, because the first
    // states arrive before this view has a window to title.
    applyAppearance()
  }
}
